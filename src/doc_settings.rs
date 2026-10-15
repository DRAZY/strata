//! The settings one file gets: settings.json, then the syntax's own
//! settings (Strata's built-in ones, then `syntax_settings`), then
//! `.editorconfig`. The file's detected indentation and choices made for
//! the tab (status bar, View ▸ Word Wrap) are layered on in `app::doc_settings`.

use crate::buffer::LineEnding;
use crate::editorconfig::Properties;
use crate::encoding::Encoding;
use crate::session::Settings;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Settings for one syntax (`"syntax_settings": {"Python": {…}}`): each one
/// left out keeps the value from settings.json.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct Layer {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tab_size: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub translate_tabs_to_spaces: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detect_indentation: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub word_wrap: Option<bool>,
    /// 0 wraps at the window edge.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wrap_column: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wrap_width: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rulers: Option<Vec<usize>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trim_trailing_white_space_on_save: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ensure_newline_at_eof_on_save: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_indent: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub smart_indent: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spell_check: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_complete: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub draw_white_space: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub highlight_trailing_whitespace: Option<bool>,
}

/// Syntaxes that soft-wrap unless told otherwise.
pub const PROSE: &[&str] = &["Markdown", "MultiMarkdown", "Plain Text", "reStructuredText", "Textile", "LaTeX"];

/// Strata's own per-syntax defaults, under the user's `syntax_settings`
/// (and over settings.json, as a syntax's settings are).
pub fn built_in(syntax: &str) -> Layer {
    match syntax {
        // Recipes must start with a tab.
        "Makefile" => Layer { translate_tabs_to_spaces: Some(false), detect_indentation: Some(false), ..Default::default() },
        // gofmt indents with tabs.
        "Go" => Layer { translate_tabs_to_spaces: Some(false), ..Default::default() },
        "YAML" => Layer { tab_size: Some(2), translate_tabs_to_spaces: Some(true), ..Default::default() },
        _ if PROSE.contains(&syntax) => Layer { word_wrap: Some(true), ..Default::default() },
        _ => Layer::default(),
    }
}

/// The user's `syntax_settings` entry for `syntax` (names compare ignoring case).
pub fn user_layer<'a>(s: &'a Settings, syntax: &str) -> Option<(&'a str, &'a Layer)> {
    s.syntax_settings.iter().find(|(k, _)| k.eq_ignore_ascii_case(syntax)).map(|(k, l)| (k.as_str(), l))
}

/// Where a file's indentation comes from (shown in the status bar's indentation menu).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Source {
    #[default]
    Settings,
    /// Strata's built-in settings for this syntax.
    BuiltIn(String),
    /// `syntax_settings` (the key as written).
    Syntax(String),
    /// Followed the file's own indentation.
    Detected,
    /// `.editorconfig`.
    EditorConfig,
    /// Chosen for this tab (status bar, View ▸ Indentation, Guess Settings From Buffer).
    Tab,
}

impl Source {
    pub fn describe(&self) -> String {
        match self {
            Source::Settings => "From settings".into(),
            Source::BuiltIn(s) => format!("Strata's default for {s}"),
            Source::Syntax(s) => format!("From syntax_settings ({s})"),
            Source::Detected => "Detected from the file".into(),
            Source::EditorConfig => "From .editorconfig".into(),
            Source::Tab => "Set for this tab".into(),
        }
    }
}

/// Resolved settings for one file.
#[derive(Clone, Debug, PartialEq)]
pub struct Effective {
    pub tab_size: usize,
    pub translate_tabs_to_spaces: bool,
    pub detect_indentation: bool,
    pub word_wrap: bool,
    pub wrap_column: Option<usize>,
    pub wrap_width: usize,
    pub rulers: Vec<usize>,
    pub trim_trailing_white_space_on_save: bool,
    pub ensure_newline_at_eof_on_save: bool,
    pub auto_indent: bool,
    pub smart_indent: bool,
    pub spell_check: bool,
    pub auto_complete: bool,
    pub draw_white_space: String,
    pub highlight_trailing_whitespace: bool,
    /// Where `tab_size` / `translate_tabs_to_spaces` came from.
    pub indent_from: Source,
    /// `.editorconfig`'s indent_style (true: spaces) and width, which beat detection.
    pub ec_spaces: Option<bool>,
    pub ec_width: Option<usize>,
    /// `.editorconfig`'s end_of_line and charset.
    pub end_of_line: Option<LineEnding>,
    pub charset: Option<Encoding>,
    /// The `.editorconfig` files that applied.
    pub editorconfig: Vec<PathBuf>,
}

impl Effective {
    fn from_settings(s: &Settings) -> Self {
        Self {
            tab_size: s.tab_size,
            translate_tabs_to_spaces: s.translate_tabs_to_spaces,
            detect_indentation: s.detect_indentation,
            word_wrap: s.word_wrap,
            wrap_column: s.wrap_column,
            wrap_width: s.wrap_width,
            rulers: s.rulers.clone(),
            trim_trailing_white_space_on_save: s.trim_trailing_white_space_on_save,
            ensure_newline_at_eof_on_save: s.ensure_newline_at_eof_on_save,
            auto_indent: s.auto_indent,
            smart_indent: s.smart_indent,
            spell_check: s.spell_check,
            auto_complete: s.auto_complete,
            draw_white_space: s.draw_white_space.clone(),
            highlight_trailing_whitespace: s.highlight_trailing_whitespace,
            indent_from: Source::Settings,
            ec_spaces: None,
            ec_width: None,
            end_of_line: None,
            charset: None,
            editorconfig: Vec::new(),
        }
    }

    fn apply(&mut self, l: &Layer, from: Source) {
        fn set<T: Clone>(to: &mut T, v: &Option<T>) {
            if let Some(v) = v {
                *to = v.clone();
            }
        }
        if l.tab_size.is_some() || l.translate_tabs_to_spaces.is_some() {
            self.indent_from = from;
        }
        set(&mut self.tab_size, &l.tab_size.filter(|&n| n > 0));
        set(&mut self.translate_tabs_to_spaces, &l.translate_tabs_to_spaces);
        set(&mut self.detect_indentation, &l.detect_indentation);
        set(&mut self.word_wrap, &l.word_wrap);
        if let Some(c) = l.wrap_column {
            self.wrap_column = (c > 0).then_some(c);
        }
        set(&mut self.wrap_width, &l.wrap_width);
        set(&mut self.rulers, &l.rulers);
        set(&mut self.trim_trailing_white_space_on_save, &l.trim_trailing_white_space_on_save);
        set(&mut self.ensure_newline_at_eof_on_save, &l.ensure_newline_at_eof_on_save);
        set(&mut self.auto_indent, &l.auto_indent);
        set(&mut self.smart_indent, &l.smart_indent);
        set(&mut self.spell_check, &l.spell_check);
        set(&mut self.auto_complete, &l.auto_complete);
        set(&mut self.draw_white_space, &l.draw_white_space);
        set(&mut self.highlight_trailing_whitespace, &l.highlight_trailing_whitespace);
    }

    fn apply_editorconfig(&mut self, ec: &Properties) {
        self.ec_spaces = ec.use_spaces();
        self.ec_width = ec.width();
        set_opt(&mut self.trim_trailing_white_space_on_save, ec.trim_trailing_whitespace());
        set_opt(&mut self.ensure_newline_at_eof_on_save, ec.insert_final_newline());
        if let Some(n) = ec.max_line_length() {
            // A guide at the limit; Wrap Paragraph and a soft wrap there too.
            self.rulers = vec![n];
            self.wrap_width = n;
            self.wrap_column = Some(n);
        }
        self.end_of_line = ec.end_of_line();
        self.charset = ec.charset();
        self.editorconfig = ec.files.clone();
    }

    /// The file's indentation, as (spaces?, width) and where it came from:
    /// `.editorconfig` beats the indentation `detected` in the file (spaces?,
    /// and the width when spaces showed one), which beats the settings.
    pub fn indentation(&self, detected: Option<(bool, Option<usize>)>) -> (bool, usize, Source) {
        let detected = detected.filter(|_| self.detect_indentation);
        let mut from = self.indent_from.clone();
        let spaces = match (self.ec_spaces, detected) {
            (Some(s), _) => {
                from = Source::EditorConfig;
                s
            }
            (None, Some((s, _))) => {
                from = Source::Detected;
                s
            }
            (None, None) => self.translate_tabs_to_spaces,
        };
        let width = match (self.ec_width, detected.and_then(|d| d.1)) {
            (Some(w), _) => {
                from = Source::EditorConfig;
                w
            }
            (None, Some(w)) => {
                if from != Source::EditorConfig {
                    from = Source::Detected;
                }
                w
            }
            (None, None) => self.tab_size,
        };
        (spaces, width.max(1), from)
    }
}

impl Default for Effective {
    fn default() -> Self {
        Self::from_settings(&Settings::default())
    }
}

/// One tab's resolved settings and what was chosen for it alone.
#[derive(Clone, Debug, Default)]
pub struct Local {
    /// What `effective` was resolved from: (settings rev, .editorconfig rev, syntax, path).
    pub key: Option<(u64, u64, String, Option<PathBuf>)>,
    pub effective: Effective,
    /// Indentation found in the file when it opened: spaces?, and the width
    /// when spaces showed one.
    pub detected: Option<(bool, Option<usize>)>,
    pub detection_done: bool,
    /// The indentation last given to the editor: a different one there was set by hand.
    pub applied_indent: Option<(bool, usize)>,
    /// Indentation set by hand for this tab, which beats everything else.
    pub tab_indent: Option<(bool, usize)>,
    pub indent_from: Source,
    /// Spell check turned on or off for this tab (View ▸ Spell Check).
    pub spell_check: Option<bool>,
    /// The line ending was picked in the status bar: `.editorconfig` doesn't convert it on save.
    pub chose_line_ending: bool,
}

/// A value that only settings.json and the syntax's settings decide
/// (`.editorconfig` has no say), worked out without resolving everything.
pub fn syntax_value<T>(s: &Settings, syntax: &str, get: impl Fn(&Layer) -> Option<T>, default: T) -> T {
    user_layer(s, syntax).and_then(|(_, l)| get(l)).or_else(|| get(&built_in(syntax))).unwrap_or(default)
}

fn set_opt(to: &mut bool, v: Option<bool>) {
    if let Some(v) = v {
        *to = v;
    }
}

/// settings.json < built-in syntax defaults < `syntax_settings` < `.editorconfig`
/// (when the `editorconfig` setting is on).
pub fn resolve(s: &Settings, syntax: &str, ec: Option<&Properties>) -> Effective {
    let mut e = Effective::from_settings(s);
    e.apply(&built_in(syntax), Source::BuiltIn(syntax.to_owned()));
    if let Some((key, layer)) = user_layer(s, syntax) {
        e.apply(layer, Source::Syntax(key.to_owned()));
    }
    if let Some(ec) = ec.filter(|_| s.editorconfig) {
        e.apply_editorconfig(ec);
    }
    e
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::Arc;

    fn settings(json: &str) -> Settings {
        Settings::parse(json).unwrap()
    }

    fn ec(text: &str) -> Properties {
        Properties::resolve(Path::new("/p/a.py"), &[(PathBuf::from("/p"), Arc::new(crate::editorconfig::File::parse(text)))])
    }

    #[test]
    fn precedence_chain() {
        let s = settings(r#"{ "tab_size": 8, "rulers": [100], "syntax_settings": { "python": { "tab_size": 3, "rulers": [79] } } }"#);
        // settings.json alone.
        let e = resolve(&s, "Rust", None);
        assert_eq!((e.tab_size, e.rulers.clone(), e.indent_from.clone()), (8, vec![100], Source::Settings));
        // syntax_settings beat it (the name ignores case).
        let e = resolve(&s, "Python", None);
        assert_eq!((e.tab_size, e.rulers.clone(), e.indent_from.clone()), (3, vec![79], Source::Syntax("python".into())));
        // .editorconfig beats both.
        let e = resolve(&s, "Python", Some(&ec("[*]\nindent_size = 2\nmax_line_length = 88\n")));
        assert_eq!((e.indentation(None).1, e.rulers.clone(), e.wrap_column, e.wrap_width), (2, vec![88], Some(88), 88));
        assert_eq!(e.indentation(None).2, Source::EditorConfig);
        assert_eq!(e.editorconfig, vec![PathBuf::from("/p/.editorconfig")]);
        // …unless the editorconfig setting is off.
        let off = settings(r#"{ "editorconfig": false }"#);
        let e = resolve(&off, "Python", Some(&ec("[*]\nindent_size = 2\n")));
        assert_eq!(e.indentation(None).1, 4);
        assert!(e.editorconfig.is_empty());
    }

    #[test]
    fn built_in_defaults_sit_between_settings_and_syntax_settings() {
        let s = settings(r#"{ "tab_size": 8, "translate_tabs_to_spaces": true }"#);
        let e = resolve(&s, "YAML", None);
        assert_eq!((e.tab_size, e.translate_tabs_to_spaces, e.indent_from.clone()), (2, true, Source::BuiltIn("YAML".into())));
        let e = resolve(&s, "Go", None);
        assert_eq!((e.tab_size, e.translate_tabs_to_spaces), (8, false), "tabs, at the user's width");
        assert!(!resolve(&s, "Makefile", None).translate_tabs_to_spaces);
        assert!(resolve(&s, "Markdown", None).word_wrap && !resolve(&s, "Rust", None).word_wrap);
        let s = settings(r#"{ "syntax_settings": { "YAML": { "tab_size": 4 }, "Markdown": { "word_wrap": false, "spell_check": true } } }"#);
        assert_eq!(resolve(&s, "YAML", None).tab_size, 4);
        let md = resolve(&s, "Markdown", None);
        assert!(!md.word_wrap && md.spell_check);
    }

    #[test]
    fn detection_sits_between_syntax_and_editorconfig() {
        let s = settings(r#"{ "syntax_settings": { "Python": { "tab_size": 3 } } }"#);
        let e = resolve(&s, "Python", None);
        assert_eq!(e.indentation(Some((true, Some(2)))), (true, 2, Source::Detected));
        // Tabs detected: the width stays the setting's.
        assert_eq!(e.indentation(Some((false, None))), (false, 3, Source::Detected));
        assert_eq!(e.indentation(None), (true, 3, Source::Syntax("Python".into())));
        let e = resolve(&s, "Python", Some(&ec("[*]\nindent_style = tab\n")));
        assert_eq!(e.indentation(Some((true, Some(2)))), (false, 2, Source::EditorConfig), "editorconfig's style, detection's width");
        let e = resolve(&s, "Python", Some(&ec("[*]\nindent_style = space\nindent_size = 4\n")));
        assert_eq!(e.indentation(Some((false, None))), (true, 4, Source::EditorConfig));
        // detect_indentation off (here for Makefiles): detection is ignored.
        let e = resolve(&s, "Makefile", None);
        assert_eq!(e.indentation(Some((true, Some(2)))), (false, 4, Source::BuiltIn("Makefile".into())));
    }

    #[test]
    fn editorconfig_save_options_and_unknown_values() {
        let s = settings(r#"{ "trim_trailing_white_space_on_save": true, "syntax_settings": { "Python": { "ensure_newline_at_eof_on_save": true } } }"#);
        let e = resolve(&s, "Python", Some(&ec("[*]\ntrim_trailing_whitespace = false\nend_of_line = crlf\ncharset = latin1\n")));
        assert!(!e.trim_trailing_white_space_on_save && e.ensure_newline_at_eof_on_save);
        assert_eq!((e.end_of_line, e.charset), (Some(LineEnding::CrLf), Some(Encoding::ISO_8859_1)));
        let e = resolve(&s, "Python", Some(&ec("[*]\ninsert_final_newline = false\nmax_line_length = off\n")));
        assert!(e.trim_trailing_white_space_on_save && !e.ensure_newline_at_eof_on_save);
        assert_eq!(e.rulers, Vec::<usize>::new());
    }

    #[test]
    fn syntax_settings_parse_and_wrap_column_zero_means_the_edge() {
        let s = settings(r#"{ "wrap_column": 100, "syntax_settings": { "Markdown": { "wrap_column": 0, "draw_white_space": "all", "unknown": 1 } } }"#);
        let e = resolve(&s, "Markdown", None);
        assert_eq!((e.wrap_column, e.draw_white_space.as_str()), (None, "all"));
        assert!(Settings::parse(r#"{ "syntax_settings": { "Python": { "tab_size": "four" } } }"#).is_err(), "a wrong type is an error, like any setting");
    }
}
