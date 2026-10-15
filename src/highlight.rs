//! Incremental syntax highlighting via syntect (reads .sublime-syntax / .tmTheme).
//!
//! Parser state is checkpointed every `CHUNK` lines. Highlighting a visible
//! line resumes from the nearest checkpoint, and an edit only discards state
//! from the edited line down. Work per frame is capped (lines and time) so a
//! huge file never freezes the UI: unfinished lines render plain and fill in
//! on later frames. Lines far past the last checkpoint (a jump into the
//! middle of a big file) are drawn at once from a guessed state while a
//! worker thread parses up to them; its checkpoints then replace the guess.

use crate::buffer::Buffer;
use egui::Color32;
use std::collections::HashMap;
use std::ops::{Range, RangeInclusive};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};
use syntect::highlighting::{
    FontStyle, HighlightIterator, HighlightState, Highlighter as ThemeHighlighter, Theme, ThemeSet,
};
use syntect::parsing::syntax_definition::Pattern;
use syntect::parsing::{ParseState, Regex, ScopeStack, ScopeStackOp, SyntaxDefinition, SyntaxReference, SyntaxSet};

pub mod scheme;
pub mod themes;

pub use scheme::Adjustments;
pub use themes::Kind;

const CHUNK: usize = 32;
/// Lines parsed per frame at most before yielding…
const BUDGET: usize = 4_000;
/// …or once this much time has gone (`prepare`; see `prepare_within`), but
/// never fewer than `MIN_LINES`. Some grammars take a millisecond a line, so
/// a screenful may fill in over a few frames rather than stall one.
pub const TIME_BUDGET: Duration = Duration::from_millis(5);
const MIN_LINES: usize = 2;
/// Visible lines further than this past the last checkpoint are guessed
/// (and parsed for real on a worker) rather than parsed up to on the UI thread.
const FAR: usize = 256;
/// Unit tests check results, not timing: per-frame time budgets don't apply
/// to them (a slow debug build would otherwise yield early). The benchmark
/// turns this off to measure what users get.
#[cfg(test)]
pub static UNTIMED: AtomicBool = AtomicBool::new(true);

/// `budget`, or (in unit tests) no limit; see `UNTIMED`.
pub fn time_budget(budget: Duration) -> Duration {
    #[cfg(test)]
    if UNTIMED.load(Ordering::Relaxed) {
        return Duration::from_secs(3600);
    }
    budget
}

/// How far above the first visible line a guess looks for an unindented line to start at.
const GUESS_BACK: usize = 40;
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
static THEMES: LazyLock<Registry> = LazyLock::new(|| {
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

/// One color scheme: the syntect theme plus what Strata draws beyond it.
#[derive(Clone, Debug)]
pub struct Scheme {
    pub theme: Theme,
    pub kind: Kind,
    /// Visible whitespace marks.
    pub whitespace: Option<syntect::highlighting::Color>,
    /// Terminal red, green, yellow, blue, magenta, cyan.
    pub ansi: Option<[Color32; 6]>,
}

/// How the theme menus group schemes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Group {
    Dark,
    Light,
    HighContrast,
    /// syntect's built-in schemes.
    Classic,
    /// From the config folder's `Themes/`.
    User,
}

impl Group {
    pub fn label(self) -> &'static str {
        match self {
            Group::Dark => "Dark",
            Group::Light => "Light",
            Group::HighContrast => "High Contrast",
            Group::Classic => "Classic",
            Group::User => "Your Themes",
        }
    }
}

struct Entry {
    name: String,
    group: Group,
    kind: Kind,
    /// Curated palettes are built the first time they're shown.
    spec: Option<&'static themes::Spec>,
    built: OnceLock<Arc<Scheme>>,
}

impl Entry {
    fn scheme(&self) -> &Arc<Scheme> {
        self.built.get_or_init(|| Arc::new(themes::build(self.spec.expect("unbuilt entries have a palette"))))
    }
}

/// Every color scheme, in menu order.
pub struct Registry {
    entries: Vec<Entry>,
}

impl Registry {
    fn get(&self, name: &str) -> Option<&Entry> {
        let name = match name {
            // syntect's Solarized builds are replaced by the curated ones.
            "Solarized (dark)" => "Solarized Dark",
            "Solarized (light)" => "Solarized Light",
            n => n,
        };
        self.entries.iter().find(|e| e.name == name).or_else(|| self.entries.iter().find(|e| e.name.eq_ignore_ascii_case(name)))
    }

    #[cfg(test)]
    pub fn contains(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    fn insert(&mut self, name: String, group: Group, scheme: Scheme) {
        let entry = Entry { name, group, kind: scheme.kind, spec: None, built: OnceLock::from(Arc::new(scheme)) };
        match self.entries.iter_mut().find(|e| e.name == entry.name) {
            // A user scheme with a bundled name replaces it.
            Some(e) => *e = entry,
            None => self.entries.push(entry),
        }
    }

    fn insert_spec(&mut self, group: Group, spec: &'static themes::Spec) {
        self.entries.push(Entry { name: spec.name.into(), group, kind: spec.kind, spec: Some(spec), built: OnceLock::new() });
    }
}

pub fn rgb_color(c: Color32) -> syntect::highlighting::Color {
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    syntect::highlighting::Color { r, g, b, a }
}

/// The bundled color schemes plus every `.tmTheme` and `.sublime-color-scheme`
/// in `dir` (named after the file).
pub fn load_themes(dir: Option<&Path>) -> (Registry, Vec<String>) {
    let mut reg = Registry { entries: Vec::new() };
    let strata = |src: &str| {
        let theme = ThemeSet::load_from_reader(&mut std::io::Cursor::new(src)).expect("bundled theme parses");
        let kind = scheme::kind_of(&theme);
        Scheme { theme, kind, whitespace: None, ansi: None }
    };
    reg.insert("Strata Dark".into(), Group::Dark, strata(include_str!("../assets/themes/Strata Dark.tmTheme")));
    for spec in themes::SPECS.iter().filter(|s| s.kind == Kind::Dark) {
        reg.insert_spec(Group::Dark, spec);
    }
    reg.insert("Strata Light".into(), Group::Light, strata(include_str!("../assets/themes/Strata Light.tmTheme")));
    for spec in themes::SPECS.iter().filter(|s| s.kind == Kind::Light) {
        reg.insert_spec(Group::Light, spec);
    }
    for spec in themes::SPECS.iter().filter(|s| s.kind.is_high_contrast()) {
        reg.insert_spec(Group::HighContrast, spec);
    }
    let mut classic: Vec<(String, Theme)> = ThemeSet::load_defaults().themes.into_iter().filter(|(n, _)| !n.starts_with("Solarized")).collect();
    classic.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, theme) in classic {
        let kind = scheme::kind_of(&theme);
        reg.insert(name, Group::Classic, Scheme { theme, kind, whitespace: None, ansi: None });
    }
    let mut errors = Vec::new();
    let Some(dir) = dir else { return (reg, errors) };
    let mut files = files_with_extension(dir, "tmTheme");
    files.extend(files_with_extension(dir, "sublime-color-scheme"));
    files.sort();
    for path in files {
        let Some(name) = path.file_stem().and_then(|s| s.to_str()) else { continue };
        let scheme = if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("tmTheme")) {
            ThemeSet::get_theme(&path).map_err(|e| e.to_string()).map(|theme| {
                let kind = scheme::kind_of(&theme);
                Scheme { theme, kind, whitespace: None, ansi: None }
            })
        } else {
            std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|text| scheme::import_sublime_color_scheme(&text, name)).map(|(s, warnings)| {
                if let Some(first) = warnings.first() {
                    let more = if warnings.len() > 1 { format!(" (+{} more)", warnings.len() - 1) } else { String::new() };
                    errors.push(format!("Theme {name}: {first}{more}"));
                }
                s
            })
        };
        match scheme {
            Ok(s) => reg.insert(name.to_owned(), Group::User, s),
            Err(e) => errors.push(format!("Theme {name}: {e}")),
        }
    }
    (reg, errors)
}

/// User customizations (settings `theme_overrides`, `font_options`) and a
/// generation number bumped whenever they change.
#[cfg(not(test))]
static ADJUST: LazyLock<RwLock<(u64, Adjustments)>> = LazyLock::new(|| RwLock::new((0, Adjustments::default())));
/// Schemes with the current adjustments applied, by name, with the
/// generation they were built for.
type AdjustedCache = HashMap<String, (u64, Arc<Scheme>)>;
#[cfg(not(test))]
static ADJUSTED: LazyLock<Mutex<AdjustedCache>> = LazyLock::new(Default::default);

// Tests run in parallel threads, each with its own app: give each thread its
// own customizations so one test's overrides can't leak into another's.
#[cfg(test)]
thread_local! {
    static ADJUST_T: &'static RwLock<(u64, Adjustments)> = Box::leak(Box::new(RwLock::new((0, Adjustments::default()))));
    static ADJUSTED_T: &'static Mutex<AdjustedCache> = Box::leak(Box::default());
}
#[cfg(test)]
struct PerThread<T: 'static>(&'static std::thread::LocalKey<&'static T>);
#[cfg(test)]
impl<T> std::ops::Deref for PerThread<T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.0.with(|t| *t)
    }
}
#[cfg(test)]
static ADJUST: PerThread<RwLock<(u64, Adjustments)>> = PerThread(&ADJUST_T);
#[cfg(test)]
static ADJUSTED: PerThread<Mutex<AdjustedCache>> = PerThread(&ADJUSTED_T);

/// Replace the user's customizations. Returns true if they changed (every
/// highlighter then picks up the new colors on its next `set_theme`).
pub fn set_adjustments(adj: Adjustments) -> bool {
    let mut cur = ADJUST.write().unwrap_or_else(|e| e.into_inner());
    if cur.1 == adj {
        return false;
    }
    cur.0 += 1;
    cur.1 = adj;
    true
}

fn generation() -> u64 {
    ADJUST.read().unwrap_or_else(|e| e.into_inner()).0
}

/// The scheme called `name` with the user's adjustments, if it exists.
pub fn scheme(name: &str) -> Option<Arc<Scheme>> {
    let entry = THEMES.get(name)?;
    let guard = ADJUST.read().unwrap_or_else(|e| e.into_inner());
    let (generation, adj) = &*guard;
    if adj.is_empty() {
        return Some(entry.scheme().clone());
    }
    let mut cache = ADJUSTED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((g, s)) = cache.get(&entry.name)
        && g == generation
    {
        return Some(s.clone());
    }
    let s = Arc::new(scheme::adjusted(entry.scheme(), &entry.name, adj));
    cache.insert(entry.name.clone(), (*generation, s.clone()));
    Some(s)
}

fn scheme_or_default(name: &str) -> Arc<Scheme> {
    scheme(name).or_else(|| scheme(DEFAULT_THEME)).expect("the default scheme exists")
}

/// The canonical name for a scheme setting (case and old names forgiven).
pub fn theme_canonical_name(name: &str) -> Option<&'static str> {
    THEMES.get(name).map(|e| e.name.as_str())
}

/// Light, dark or high contrast, if the scheme exists.
pub fn theme_kind(name: &str) -> Option<Kind> {
    THEMES.get(name).map(|e| e.kind)
}

/// Scheme names by menu group, in menu order.
pub fn theme_groups() -> Vec<(Group, Vec<&'static str>)> {
    let mut out: Vec<(Group, Vec<&'static str>)> = Vec::new();
    for group in [Group::Dark, Group::Light, Group::HighContrast, Group::User, Group::Classic] {
        let names: Vec<&'static str> = THEMES.entries.iter().filter(|e| e.group == group).map(|e| e.name.as_str()).collect();
        if !names.is_empty() {
            out.push((group, names));
        }
    }
    out
}

/// The menu group of a scheme.
pub fn theme_group(name: &str) -> Option<Group> {
    THEMES.get(name).map(|e| e.group)
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

/// Every color scheme name, in menu order (dark, light, high contrast,
/// yours, classic).
pub fn theme_names() -> Vec<&'static str> {
    theme_groups().into_iter().flat_map(|(_, names)| names).collect()
}

/// Syntax-highlighted HTML (inline styles) for `text`, for Copy as HTML.
pub fn to_html(text: &str, syntax: &str, theme: &str) -> String {
    let syntax = SYNTAXES.find_syntax_by_name(syntax).unwrap_or_else(|| SYNTAXES.find_syntax_plain_text());
    let scheme = scheme_or_default(theme);
    syntect::html::highlighted_html_for_string(text, &SYNTAXES, syntax, &scheme.theme).unwrap_or_else(|_| format!("<pre>{text}</pre>"))
}

/// The theme's foreground for a scope like "markup.heading", if it sets one
/// (Markdown preview colors).
pub fn scope_color(theme: &str, scope: &str) -> Option<Color32> {
    let scheme = scheme(theme)?;
    let stack = <ScopeStack as std::str::FromStr>::from_str(scope).ok()?;
    let hl = ThemeHighlighter::new(&scheme.theme);
    let style = hl.style_for_stack(stack.as_slice());
    let fg = scheme.theme.settings.foreground?;
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
    let scheme = scheme_or_default(theme);
    let mut h = syntect::easy::HighlightLines::new(syntax, &scheme.theme);
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
    scheme: Arc<Scheme>,
    theme_name: String,
    /// The adjustments generation `scheme` was built with.
    generation: u64,
    syntax_name: String,
    /// `checkpoints[k]` = state at the start of line `k * CHUNK`.
    checkpoints: Vec<Checkpoint>,
    lines: HashMap<usize, LineSpans>,
    /// Lines highlighted from a guessed state, until the real parse gets there.
    guessed: HashMap<usize, LineSpans>,
    /// Parses ahead of the checkpoints on a worker thread.
    indexer: Option<Indexer>,
    /// The last `prepare` showed guesses and is waiting for the worker.
    waiting: bool,
    /// Where a `prepare` that ran out of time stopped: the next one goes on
    /// from there instead of re-parsing from the checkpoint.
    resume: Option<(usize, Checkpoint)>,
    /// Compile a grammar's regexes off the UI thread the first time it is used.
    warm_up: bool,
    /// Spell check's scope selector; with one, highlighting a line also
    /// records which of its bytes are prose (`prose`).
    prose_selector: Option<Arc<crate::spell::Selector>>,
    prose: HashMap<usize, Arc<Vec<Range<usize>>>>,
}

/// Parser state and scope stack at the start of a line. The highlight state
/// is rebuilt from the stack, so checkpoints don't depend on the theme.
type Checkpoint = (ParseState, ScopeStack);

/// A worker parsing the text from one checkpoint to the end, publishing a
/// checkpoint every `CHUNK` lines. Dropping it stops the thread.
struct Indexer {
    cancel: Arc<AtomicBool>,
    out: Arc<Mutex<Indexed>>,
    /// Its checkpoints up to this index still hold: later ones parsed text
    /// that has been edited since.
    valid: usize,
}

#[derive(Default)]
struct Indexed {
    /// Index (in `Highlighter::checkpoints`) of `states[0]`.
    from: usize,
    states: Vec<Checkpoint>,
    done: bool,
}

impl Drop for Indexer {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Indexer {
    /// Parse `rope` from checkpoint `k` (state `start`) on.
    fn spawn(rope: ropey::Rope, k: usize, start: Checkpoint) -> Self {
        let cancel = Arc::new(AtomicBool::new(false));
        let out = Arc::new(Mutex::new(Indexed { from: k + 1, ..Default::default() }));
        let (stop, sink) = (cancel.clone(), out.clone());
        std::thread::spawn(move || {
            let (mut parse, mut stack) = start;
            let total = rope.len_lines();
            let publish = |batch: &mut Vec<Checkpoint>, done: bool| {
                let mut out = sink.lock().unwrap_or_else(|e| e.into_inner());
                out.states.append(batch);
                out.done = done;
            };
            let mut batch = Vec::new();
            let mut text = String::new();
            let mut line = k * CHUNK;
            while line + CHUNK < total {
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                for l in line..line + CHUNK {
                    text.clear();
                    rope.line(l).chunks().for_each(|c| text.push_str(c));
                    // As `Buffer::line_text` + `highlight_line`: no line break, then the '\n' grammars expect.
                    text.truncate(text.trim_end_matches(['\n', '\r']).len());
                    if text.len() > MAX_LINE {
                        continue;
                    }
                    text.push('\n');
                    if let Ok(ops) = parse.parse_line(&text, &SYNTAXES) {
                        for (_, op) in &ops {
                            let _ = stack.apply(op);
                        }
                    }
                }
                line += CHUNK;
                batch.push((parse.clone(), stack.clone()));
                if batch.len() >= 64 {
                    publish(&mut batch, false);
                }
            }
            publish(&mut batch, true);
        });
        Self { cancel, out, valid: usize::MAX }
    }
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
            scheme: scheme_or_default(DEFAULT_THEME),
            theme_name: DEFAULT_THEME.into(),
            generation: generation(),
            syntax_name: "Plain Text".into(),
            checkpoints: Vec::new(),
            lines: HashMap::new(),
            guessed: HashMap::new(),
            indexer: None,
            waiting: false,
            resume: None,
            warm_up: false,
            prose_selector: None,
            prose: HashMap::new(),
        }
    }

    /// Start (or stop) recording prose ranges for spell check. A different
    /// selector re-highlights the lines shown.
    pub fn set_prose_selector(&mut self, selector: Option<Arc<crate::spell::Selector>>) {
        let same = match (&self.prose_selector, &selector) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if !same {
            self.prose_selector = selector;
            self.lines.clear();
            self.prose.clear();
        }
    }

    /// Byte ranges of `line` matching the prose selector, once highlighted.
    pub fn prose(&self, line: usize) -> Option<Arc<Vec<Range<usize>>>> {
        self.prose.get(&line).cloned()
    }

    /// Prose ranges of the lines from `from` on, parsing from the nearest
    /// checkpoint (lines not yet highlighted included); `f` gets each line's
    /// number, text and ranges and returns false to stop.
    pub fn prose_lines(&self, buffer: &Buffer, from: usize, mut f: impl FnMut(usize, &str, &[Range<usize>]) -> bool) {
        let Some(selector) = self.prose_selector.clone() else { return };
        let (mut parse, mut stack, start) = match self.checkpoints.len() {
            0 => (ParseState::new(self.syntax()), ScopeStack::new(), 0),
            len => {
                let k = (from / CHUNK).min(len - 1);
                let (p, h) = &self.checkpoints[k];
                (p.clone(), h.clone(), k * CHUNK)
            }
        };
        for line in start..buffer.len_lines() {
            let text = buffer.line_text(line);
            let prose = if text.len() > MAX_LINE {
                Vec::new()
            } else {
                let Ok(ops) = parse.parse_line(&format!("{text}\n"), &SYNTAXES) else { return };
                prose_ranges(&selector, &mut stack, &ops, text.len())
            };
            if line >= from && !f(line, &text, &prose) {
                return;
            }
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
        self.scheme.theme
            .settings
            .background
            .map(to_color)
            .unwrap_or(Color32::from_rgb(0x2b, 0x30, 0x3b))
    }

    pub fn foreground(&self) -> Color32 {
        self.scheme.theme
            .settings
            .foreground
            .map(to_color)
            .unwrap_or(Color32::LIGHT_GRAY)
    }

    pub fn selection(&self) -> Color32 {
        self.scheme.theme
            .settings
            .selection
            .map(to_color)
            .unwrap_or(Color32::from_rgb(0x4f, 0x5b, 0x66))
    }

    /// Background of the line(s) holding a cursor.
    pub fn line_highlight(&self) -> Color32 {
        self.scheme.theme.settings.line_highlight.map(to_color).unwrap_or(Color32::from_white_alpha(8))
    }

    pub fn gutter_foreground(&self) -> Option<Color32> {
        self.scheme.theme.settings.gutter_foreground.map(to_color)
    }

    /// Indent guides.
    pub fn guide(&self) -> Color32 {
        self.scheme.theme.settings.guide.map(to_color).unwrap_or_else(|| self.foreground().gamma_multiply(0.12))
    }

    pub fn caret(&self) -> Color32 {
        self.scheme.theme
            .settings
            .caret
            .map(to_color)
            .unwrap_or(Color32::WHITE)
    }

    pub fn theme_name(&self) -> &str {
        &self.theme_name
    }

    /// Switch color scheme (or pick up changed customizations); unknown
    /// names are ignored, and setting the current one again costs nothing.
    pub fn set_theme(&mut self, name: &str) {
        let generation = generation();
        let Some(canonical) = theme_canonical_name(name) else { return };
        if canonical == self.theme_name && generation == self.generation {
            return;
        }
        if let Some(s) = scheme(canonical) {
            self.scheme = s;
            self.theme_name = canonical.into();
            self.generation = generation;
            self.invalidate_from(0);
        }
    }

    /// Changes whenever the colors do: the scheme or the user's customizations.
    pub fn theme_key(&self) -> String {
        format!("{}#{}", self.theme_name, self.generation)
    }

    /// Whether this highlighter shows `name` with the current customizations.
    pub fn is_showing(&self, name: &str) -> bool {
        theme_canonical_name(name).is_some_and(|n| n == self.theme_name) && self.generation == generation()
    }

    /// Find-match highlight, if the scheme sets one.
    pub fn find_highlight(&self) -> Option<Color32> {
        self.scheme.theme.settings.find_highlight.map(to_color)
    }

    /// Dark, light or high contrast.
    pub fn kind(&self) -> Kind {
        self.scheme.kind
    }

    /// Highlights in the chrome (scrollbar, active tab): the scheme's accent, else its caret.
    pub fn accent(&self) -> Color32 {
        self.scheme.theme.settings.accent.map(to_color).unwrap_or_else(|| self.caret())
    }

    /// The bracket pair around the caret.
    pub fn bracket(&self) -> Color32 {
        self.scheme.theme.settings.brackets_foreground.map(to_color).unwrap_or_else(|| self.caret())
    }

    /// Visible whitespace marks.
    pub fn whitespace(&self) -> Color32 {
        self.scheme.whitespace.map(to_color).unwrap_or_else(|| self.foreground().gamma_multiply(0.35))
    }

    /// Terminal hues (red, green, yellow, blue, magenta, cyan), if the scheme has them.
    pub fn ansi(&self) -> Option<[Color32; 6]> {
        self.scheme.ansi
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
        self.scheme.kind.is_light()
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
        self.prose.retain(|&l, _| l < line);
        self.guessed.retain(|&l, _| l < line);
        // A state at line `at` depends on the lines above it only (and on the syntax).
        if line == 0 || self.resume.as_ref().is_some_and(|(at, _)| *at > line) {
            self.resume = None;
        }
        // Its work above the edit still holds (typing while it catches up
        // mustn't restart it on every key); past that it's out of date.
        if let Some(ix) = &mut self.indexer {
            ix.valid = ix.valid.min(line / CHUNK);
        }
        self.absorb();
    }

    /// Highlighted spans for a line, if ready. Call `prepare` first each frame.
    pub fn line(&self, line: usize) -> Option<LineSpans> {
        self.lines.get(&line).or_else(|| self.guessed.get(&line)).cloned()
    }

    /// True while visible lines show guessed colors and a worker parses up
    /// to them: repaint now and then rather than every frame.
    pub fn waiting(&self) -> bool {
        self.waiting
    }

    /// Take the checkpoints the worker published since the last call.
    fn absorb(&mut self) {
        let Some(ix) = &self.indexer else { return };
        let finished = {
            let mut out = ix.out.lock().unwrap_or_else(|e| e.into_inner());
            let have = self.checkpoints.len();
            if out.from <= have {
                // states[i] is checkpoint `out.from + i`: skip ones already
                // known, keep the valid rest.
                let usable = ix.valid.saturating_add(1).saturating_sub(out.from).min(out.states.len());
                let skip = (have - out.from).min(usable);
                self.checkpoints.extend(out.states.drain(..usable).skip(skip));
                out.from += usable;
            }
            out.from > ix.valid || (out.done && out.states.is_empty())
        };
        if finished {
            self.indexer = None;
        }
    }

    /// Highlight `visible` lines. Returns false if the budget ran out or
    /// some lines are guesses (the caller should repaint so highlighting
    /// continues; see `waiting`).
    pub fn prepare(&mut self, buffer: &Buffer, visible: RangeInclusive<usize>) -> bool {
        self.prepare_within(buffer, visible, TIME_BUDGET)
    }

    /// `prepare`, parsing for at most about `budget`.
    pub fn prepare_within(&mut self, buffer: &Buffer, visible: RangeInclusive<usize>, budget: Duration) -> bool {
        self.prepare_for(buffer, visible, time_budget(budget))
    }

    fn prepare_for(&mut self, buffer: &Buffer, visible: RangeInclusive<usize>, budget: Duration) -> bool {
        let last = (*visible.end()).min(buffer.len_lines().saturating_sub(1));
        let first = (*visible.start()).min(last);
        self.waiting = false;
        self.absorb();
        // Lines done on earlier calls (a range bigger than one call's budget) aren't redone.
        let Some(first) = (first..=last).find(|l| !self.lines.contains_key(l)) else { return true };
        if self.warm_up && !self.warmed_up(buffer, first.saturating_sub(CHUNK)..=last) {
            return false;
        }
        // Keep memory flat when scrolling through a huge file.
        if self.lines.len() > 8_192 {
            self.lines.retain(|&l, _| l + 2_000 >= first && l <= last + 2_000);
            self.prose.retain(|&l, _| l + 2_000 >= first && l <= last + 2_000);
        }
        if self.guessed.len() > 2_048 {
            self.guessed.retain(|&l, _| l + 500 >= first && l <= last + 500);
        }
        let scheme = self.scheme.clone();
        let hl = ThemeHighlighter::new(&scheme.theme);
        if self.checkpoints.is_empty() {
            self.checkpoints.push((ParseState::new(self.syntax()), ScopeStack::new()));
        }

        // Resume from the nearest known checkpoint at or before `first`.
        let k = (first / CHUNK).min(self.checkpoints.len() - 1);
        if first - k * CHUNK > FAR {
            if self.indexer.is_none() {
                self.indexer = Some(Indexer::spawn(buffer.snapshot(), k, self.checkpoints[k].clone()));
            }
            // Guesses done: wait for the worker (not every frame); else guess more next frame.
            self.waiting = self.guess(buffer, &hl, first, last, budget);
            return false;
        }
        // Where the last call ran out of time, if that's nearer than the checkpoint.
        let (mut line, (mut parse, stack)) = match self.resume.take() {
            Some((at, state)) if at >= k * CHUNK && at <= first => (at, state),
            _ => (k * CHUNK, self.checkpoints[k].clone()),
        };
        let mut high = HighlightState::new(&hl, stack);
        let started = Instant::now();
        let mut done = 0;
        while line <= last {
            // Only new lines count, so every call makes progress (the walk from the
            // checkpoint to `first` is under `CHUNK` lines).
            if done >= MIN_LINES && (done >= BUDGET || started.elapsed() > budget) {
                self.resume = Some((line, (parse, high.path)));
                return false;
            }
            done += usize::from(line >= first);
            if line.is_multiple_of(CHUNK) && line / CHUNK == self.checkpoints.len() {
                self.checkpoints.push((parse.clone(), high.path.clone()));
            }
            let text = buffer.line_text(line);
            let mut prose = (line >= first && self.prose_selector.is_some()).then(Vec::new);
            let spans = self.highlight_line(&hl, &mut parse, &mut high, &text, prose.as_mut());
            if line >= first {
                self.lines.insert(line, spans);
                self.guessed.remove(&line);
            }
            if let Some(p) = prose {
                self.prose.insert(line, Arc::new(p));
            }
            line += 1;
        }
        true
    }

    /// Highlight `first..=last` from a fresh parser state, started at the
    /// nearest unindented line a little above (top-level code, where a fresh
    /// state is most likely right). Within `budget` like `prepare`; returns
    /// true when every line has (at least) a guess.
    fn guess(&mut self, buffer: &Buffer, hl: &ThemeHighlighter, first: usize, last: usize, budget: Duration) -> bool {
        let known = |h: &Self, l: &usize| h.lines.contains_key(l) || h.guessed.contains_key(l);
        let Some(first) = (first..=last).find(|l| !known(self, l)) else { return true };
        let floor = first.saturating_sub(GUESS_BACK);
        let start = (floor..=first).rev().find(|&l| buffer.line_chars(l).next().is_some_and(|c| !c.is_whitespace())).unwrap_or(first);
        let mut parse = ParseState::new(self.syntax());
        let mut high = HighlightState::new(hl, ScopeStack::new());
        let started = Instant::now();
        let mut done = 0;
        for line in start..=last {
            if done >= MIN_LINES && done % 4 == 0 && started.elapsed() > budget {
                return false;
            }
            let text = buffer.line_text(line);
            let spans = self.highlight_line(hl, &mut parse, &mut high, &text, None);
            if line >= first && !self.lines.contains_key(&line) {
                done += 1;
                self.guessed.insert(line, spans);
            }
        }
        true
    }

    /// Scope names (outermost first) at char `pos`, e.g. ["source.rust",
    /// "meta.function.rust"]: what snippet `<scope>` selectors are matched
    /// against. Resumes from the nearest checkpoint; far from any, only the
    /// syntax's base scope is reported.
    pub fn scope_at(&self, buffer: &Buffer, pos: usize) -> Vec<String> {
        let syntax = self.syntax();
        let base = vec![syntax.scope.build_string()];
        let pos = pos.min(buffer.len_chars());
        let line = buffer.line_of(pos);
        let (mut parse, mut stack, from) = match self.checkpoints.len() {
            0 => (ParseState::new(syntax), ScopeStack::new(), 0),
            len => {
                let k = (line / CHUNK).min(len - 1);
                let (p, stack) = &self.checkpoints[k];
                (p.clone(), stack.clone(), k * CHUNK)
            }
        };
        if line - from > 2 * FAR {
            return base;
        }
        let upto = buffer.char_to_byte(pos) - buffer.char_to_byte(buffer.line_start(line));
        for l in from..=line {
            let text = buffer.line_text(l);
            if text.len() > MAX_LINE {
                return base;
            }
            let Ok(ops) = parse.parse_line(&format!("{text}\n"), &SYNTAXES) else { return base };
            for (at, op) in ops {
                if l == line && at > upto {
                    break;
                }
                if stack.apply(&op).is_err() {
                    return base;
                }
            }
        }
        let names: Vec<String> = stack.as_slice().iter().map(|s| s.build_string()).collect();
        if names.is_empty() { base } else { names }
    }

    fn highlight_line(
        &self,
        hl: &ThemeHighlighter,
        parse: &mut ParseState,
        high: &mut HighlightState,
        text: &str,
        prose: Option<&mut Vec<Range<usize>>>,
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
        if let (Some(out), Some(selector)) = (prose, &self.prose_selector) {
            *out = prose_ranges(selector, &mut high.path.clone(), &ops, text.len());
        }
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

/// The byte ranges of a line (`len` bytes, parsed into `ops` from `stack`)
/// whose scope matches `selector`, merged. `stack` ends as the line leaves it.
fn prose_ranges(selector: &crate::spell::Selector, stack: &mut ScopeStack, ops: &[(usize, ScopeStackOp)], len: usize) -> Vec<Range<usize>> {
    let mut out: Vec<Range<usize>> = Vec::new();
    let add = |out: &mut Vec<Range<usize>>, r: Range<usize>| match out.last_mut() {
        Some(last) if last.end == r.start => last.end = r.end,
        _ => out.push(r),
    };
    let mut pos = 0;
    for (at, op) in ops {
        let at = (*at).min(len);
        if at > pos {
            if selector.matches(stack.as_slice()) {
                add(&mut out, pos..at);
            }
            pos = at;
        }
        if stack.apply(op).is_err() {
            return out;
        }
    }
    if len > pos && selector.matches(stack.as_slice()) {
        add(&mut out, pos..len);
    }
    out
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
        while !h.prepare(&buf, 150..=170) {}
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
    fn scope_at_reports_the_scope_stack() {
        let buf = Buffer::from_str("fn main() {\n    // note\n    let s = \"hi\";\n}\n");
        let mut h = Highlighter::new();
        h.detect(Some(Path::new("a.rs")), "");
        let at = |h: &Highlighter, line: usize, col: usize| h.scope_at(&buf, buf.line_start(line) + col);
        assert_eq!(at(&h, 0, 0)[0], "source.rust");
        assert!(at(&h, 1, 7).iter().any(|s| s.starts_with("comment.line")));
        assert!(at(&h, 2, 13).iter().any(|s| s.starts_with("string.quoted")));
        // Same answer when resuming from checkpoints.
        assert!(h.prepare(&buf, 0..=3));
        assert!(at(&h, 2, 13).iter().any(|s| s.starts_with("string.quoted")));
        assert_eq!(Highlighter::new().scope_at(&buf, 3), ["text.plain"]);
    }

    #[test]
    fn bundled_strata_themes_load() {
        let mut h = Highlighter::new();
        assert_eq!(h.theme_name(), "Strata Dark");
        assert!(!h.is_light());
        h.set_theme("Strata Light");
        assert!(h.is_light());
        assert!(theme_names().contains(&"Solarized Dark"));
        // The old syntect name still works.
        h.set_theme("Solarized (dark)");
        assert_eq!(h.theme_name(), "Solarized Dark");
        h.set_theme("nord");
        assert_eq!(h.theme_name(), "Nord", "names are case-insensitive");
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

        std::fs::write(
            themes.join("Sub.sublime-color-scheme"),
            r##"{ "variables": { "ink": "#202020" }, "globals": { "background": "#fdfdfd", "foreground": "var(ink)" },
                "rules": [ { "scope": "comment", "foreground": "#777777", "font_style": "italic" }, ], }"##,
        )
        .unwrap();
        std::fs::write(themes.join("Broken.sublime-color-scheme"), "{ nope").unwrap();
        let (reg, errors) = load_themes(Some(&themes));
        assert!(reg.contains("Mine"));
        assert!(reg.contains("Strata Dark"));
        assert!(reg.contains("Sub"), ".sublime-color-scheme files import");
        assert_eq!(reg.get("Sub").map(|e| (e.group, e.kind)), Some((Group::User, Kind::Light)));
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(errors.iter().any(|e| e.contains("Junk")));
        assert!(errors.iter().any(|e| e.contains("Broken")));

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
        // Near the top: parsed on the spot, a capped amount per call.
        let mid = FAR / 2;
        let mut frames = 1;
        while !h.prepare(&buf, mid..=mid + 60) {
            frames += 1;
        }
        assert!(frames < 10 && h.lines.contains_key(&(mid + 60)));
        // Out of time: a few lines per call, and calls don't redo done lines.
        let mut calls = 1;
        while !h.prepare_for(&buf, 0..=20 * MIN_LINES, Duration::ZERO) {
            calls += 1;
            assert!(calls < 40, "{calls} calls");
        }
        assert!(calls >= 10);
        // Far down: guessed at once, exact once the worker gets there.
        let end = buf.len_lines() - 1;
        assert!(!h.prepare(&buf, end - 40..=end));
        assert!(h.waiting());
        assert!(h.line(end).is_some(), "guessed right away");
        let start = Instant::now();
        while !h.prepare(&buf, end - 40..=end) {
            assert!(start.elapsed() < Duration::from_secs(30), "the worker catches up");
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(!h.waiting() && h.lines.contains_key(&end) && h.guessed.is_empty());
    }

    #[test]
    fn edits_below_the_workers_reach_dont_restart_it() {
        let text = "let x = 1;\n".repeat(20_000);
        let mut buf = Buffer::from_str(&text);
        let mut h = Highlighter::new();
        h.detect(Some(Path::new("a.rs")), "");
        let end = buf.len_lines() - 1;
        assert!(!h.prepare(&buf, end - 20..=end));
        // Typing far down: the worker keeps going (its work above the edit holds).
        let start = Instant::now();
        loop {
            buf.insert(buf.line_start(end - 10), "y");
            h.invalidate_from(end - 10);
            if h.prepare(&buf, end - 20..=end) {
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(60), "never caught up");
            std::thread::sleep(Duration::from_millis(1));
        }
        // An edit above what it has done stops it.
        h.invalidate_from(100);
        assert!(h.indexer.is_none() && h.checkpoints.len() == 100 / CHUNK + 1);
    }

    #[test]
    fn worker_checkpoints_match_parsing_on_the_spot() {
        // A block comment across the jump: a guess from inside it would be
        // wrong; the worker's states must give exactly the colors parsing
        // straight through does.
        let mut text = String::from("fn a() {}\n/*\n");
        for i in 0..2_000 {
            text.push_str(&format!("    let x{i} = \"{i}\";\n"));
        }
        text.push_str("*/\nfn b() { let s = \"x\"; }\n");
        let buf = Buffer::from_str(&text);
        let end = buf.len_lines() - 1;
        let mut far = Highlighter::new();
        far.detect(Some(Path::new("a.rs")), "");
        let start = Instant::now();
        while !far.prepare(&buf, end - 10..=end) {
            assert!(start.elapsed() < Duration::from_secs(60));
            std::thread::sleep(Duration::from_millis(2));
        }
        let mut near = Highlighter::new();
        near.detect(Some(Path::new("a.rs")), "");
        for l in (0..=end).step_by(50) {
            while !near.prepare(&buf, l..=(l + 49).min(end)) {}
        }
        let colors = |h: &Highlighter, l: usize| h.line(l).unwrap().iter().map(|s| (s.range.clone(), s.color)).collect::<Vec<_>>();
        for l in end - 10..=end {
            assert_eq!(colors(&far, l), colors(&near, l), "line {l}");
        }
        // Inside the comment every line is one color.
        let inside = colors(&near, end - 5);
        assert!(inside.iter().all(|(_, c)| *c == inside[0].1));
    }

    /// Every scheme Strata ships defines the editor colors the view paints,
    /// and the curated ones keep text and every syntax role legible.
    #[test]
    fn bundled_schemes_are_complete_and_legible() {
        use themes::{RULES, Role, contrast};
        let (reg, errors) = load_themes(None);
        assert!(errors.is_empty());
        let groups = theme_groups();
        for g in [Group::Dark, Group::Light, Group::HighContrast] {
            assert!(groups.iter().any(|(group, names)| *group == g && !names.is_empty()), "{g:?}");
        }
        for e in &reg.entries {
            let s = e.scheme();
            let st = &s.theme.settings;
            let (bg, fg) = (to_color(st.background.expect("background")), to_color(st.foreground.expect("foreground")));
            if e.group == Group::Classic {
                continue;
            }
            for (key, c) in [("caret", st.caret), ("selection", st.selection), ("line_highlight", st.line_highlight), ("gutter_foreground", st.gutter_foreground), ("find_highlight", st.find_highlight), ("guide", st.guide)] {
                assert!(c.is_some(), "{}: no {key}", e.name);
            }
            let text = contrast(fg, bg);
            assert!(text >= 4.5, "{}: text contrast {text:.2}", e.name);
            let Some(spec) = e.spec else { continue };
            assert!(st.brackets_foreground.is_some() && s.whitespace.is_some() && s.ansi.is_some() && st.accent.is_some(), "{}", e.name);
            let (text_min, syntax_min) = themes::minimums(spec.kind);
            assert!(text >= text_min, "{}: text contrast {text:.2}", e.name);
            let hl = ThemeHighlighter::new(&s.theme);
            for (role, selectors) in RULES {
                // The role's first selector, as a scope inside a source file.
                let scope = format!("source.x {}", selectors.split(',').next().unwrap().trim());
                let stack = <ScopeStack as std::str::FromStr>::from_str(&scope).unwrap();
                let color = to_color(hl.style_for_stack(stack.as_slice()).foreground);
                let c = contrast(color, bg);
                assert!(c >= syntax_min, "{}: {role:?} contrast {c:.2}", e.name);
                if matches!(role, Role::Keyword | Role::String | Role::Comment | Role::Function | Role::Number) {
                    assert_ne!(color, fg, "{}: {role:?} isn't colored", e.name);
                }
            }
        }
    }
}
