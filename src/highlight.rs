//! Syntax highlighting via syntect (reads .sublime-syntax / .tmTheme).

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId};
use std::path::Path;
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Theme, ThemeSet};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

pub struct Highlighter {
    syntaxes: SyntaxSet,
    theme: Theme,
    syntax_name: String,
    // Re-highlighting every frame is wasteful; cache the last job by text.
    cache: Option<(String, LayoutJob)>,
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
            cache: None,
        }
    }

    pub fn background(&self) -> Color32 {
        self.theme
            .settings
            .background
            .map(to_color)
            .unwrap_or(Color32::from_rgb(0x2b, 0x30, 0x3b))
    }

    pub fn syntax_name(&self) -> &str {
        &self.syntax_name
    }

    /// Pick a syntax from the file extension, falling back to the first line (shebangs).
    pub fn detect(&mut self, path: Option<&Path>, text: &str) {
        let by_ext = path
            .and_then(|p| p.extension())
            .and_then(|e| e.to_str())
            .and_then(|e| self.syntaxes.find_syntax_by_extension(e));
        let syntax = by_ext
            .or_else(|| self.syntaxes.find_syntax_by_first_line(text))
            .unwrap_or_else(|| self.syntaxes.find_syntax_plain_text());
        self.syntax_name = syntax.name.clone();
        self.cache = None;
    }

    fn syntax(&self) -> &SyntaxReference {
        self.syntaxes
            .find_syntax_by_name(&self.syntax_name)
            .unwrap_or_else(|| self.syntaxes.find_syntax_plain_text())
    }

    pub fn layout(&mut self, text: &str, font: FontId) -> LayoutJob {
        if let Some((cached, job)) = &self.cache {
            if cached == text {
                return job.clone();
            }
        }
        let job = self.build(text, font);
        self.cache = Some((text.to_owned(), job.clone()));
        job
    }

    fn build(&self, text: &str, font: FontId) -> LayoutJob {
        let mut job = LayoutJob::default();
        job.wrap.max_width = f32::INFINITY;
        let mut h = HighlightLines::new(self.syntax(), &self.theme);
        for line in LinesWithEndings::from(text) {
            let Ok(ranges) = h.highlight_line(line, &self.syntaxes) else {
                job.append(line, 0.0, plain(font.clone()));
                continue;
            };
            for (style, piece) in ranges {
                let mut fmt = TextFormat::simple(font.clone(), to_color(style.foreground));
                fmt.italics = style.font_style.contains(FontStyle::ITALIC);
                if style.font_style.contains(FontStyle::UNDERLINE) {
                    fmt.underline = egui::Stroke::new(1.0, fmt.color);
                }
                job.append(piece, 0.0, fmt);
            }
        }
        job
    }
}

fn plain(font: FontId) -> TextFormat {
    TextFormat::simple(font, Color32::LIGHT_GRAY)
}

fn to_color(c: syntect::highlighting::Color) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a)
}
