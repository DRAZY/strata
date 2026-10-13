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
use std::path::Path;
use std::sync::{Arc, LazyLock};
use syntect::highlighting::{
    FontStyle, HighlightIterator, HighlightState, Highlighter as ThemeHighlighter, Theme, ThemeSet,
};
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet};

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

/// Loaded once and shared by every tab.
static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);
static THEMES: LazyLock<ThemeSet> = LazyLock::new(|| {
    let mut set = ThemeSet::load_defaults();
    for (name, src) in [
        ("Strata Dark", include_str!("../assets/themes/Strata Dark.tmTheme")),
        ("Strata Light", include_str!("../assets/themes/Strata Light.tmTheme")),
    ] {
        let theme = ThemeSet::load_from_reader(&mut std::io::Cursor::new(src)).expect("bundled theme parses");
        set.themes.insert(name.into(), theme);
    }
    set
});
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
    let syntax = SYNTAXES.find_syntax_by_token(lang).or_else(|| SYNTAXES.find_syntax_by_extension(&lang.to_lowercase()))?;
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

/// Syntax name for a file by extension, if it isn't plain text.
pub fn syntax_for_path(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_str()?;
    SYNTAXES.find_syntax_by_extension(ext).map(|s| s.name.clone()).filter(|n| n != "Plain Text")
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
}

impl Highlighter {
    pub fn new() -> Self {
        Self {
            theme: &THEMES.themes[DEFAULT_THEME],
            theme_name: DEFAULT_THEME.into(),
            syntax_name: "Plain Text".into(),
            checkpoints: Vec::new(),
            lines: HashMap::new(),
        }
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

    /// Pick a syntax from the file extension, falling back to the first line (shebangs).
    pub fn detect(&mut self, path: Option<&Path>, first_line: &str) {
        let by_ext = path
            .and_then(|p| p.extension())
            .and_then(|e| e.to_str())
            .and_then(|e| SYNTAXES.find_syntax_by_extension(e));
        let syntax = by_ext
            .or_else(|| SYNTAXES.find_syntax_by_first_line(first_line))
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
                let (p, h) = &self.checkpoints[k];
                (p.clone(), h.path.clone(), k * CHUNK)
            }
        };
        if line - from > 2 * BUDGET {
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
        assert!(theme_names().contains(&"Solarized (dark)"));
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
