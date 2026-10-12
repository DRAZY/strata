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
use std::sync::Arc;
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

pub struct Highlighter {
    syntaxes: SyntaxSet,
    theme: Theme,
    syntax_name: String,
    /// `checkpoints[k]` = state at the start of line `k * CHUNK`.
    checkpoints: Vec<(ParseState, HighlightState)>,
    lines: HashMap<usize, LineSpans>,
}

impl Highlighter {
    pub fn new() -> Self {
        let mut themes = ThemeSet::load_defaults();
        let theme = themes
            .themes
            .remove("base16-ocean.dark")
            .expect("bundled theme");
        Self {
            syntaxes: SyntaxSet::load_defaults_newlines(),
            theme,
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

    pub fn caret(&self) -> Color32 {
        self.theme
            .settings
            .caret
            .map(to_color)
            .unwrap_or(Color32::WHITE)
    }

    pub fn syntax_name(&self) -> &str {
        &self.syntax_name
    }

    /// Pick a syntax from the file extension, falling back to the first line (shebangs).
    pub fn detect(&mut self, path: Option<&Path>, first_line: &str) {
        let by_ext = path
            .and_then(|p| p.extension())
            .and_then(|e| e.to_str())
            .and_then(|e| self.syntaxes.find_syntax_by_extension(e));
        let syntax = by_ext
            .or_else(|| self.syntaxes.find_syntax_by_first_line(first_line))
            .unwrap_or_else(|| self.syntaxes.find_syntax_plain_text());
        self.syntax_name = syntax.name.clone();
        self.invalidate_from(0);
    }

    fn syntax(&self) -> &SyntaxReference {
        self.syntaxes
            .find_syntax_by_name(&self.syntax_name)
            .unwrap_or_else(|| self.syntaxes.find_syntax_plain_text())
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
        let hl = ThemeHighlighter::new(&self.theme);
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
            if line % CHUNK == 0 && line / CHUNK == self.checkpoints.len() {
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
        let Ok(ops) = parse.parse_line(&with_nl, &self.syntaxes) else {
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
