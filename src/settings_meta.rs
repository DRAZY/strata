//! Every setting described once, for the Settings window: its category,
//! label and the control that edits it. Descriptions, types and defaults
//! come from the manual's Settings Reference (docs/manual/settings.md), so the
//! window and the manual say the same thing; tests check that every field of
//! `Settings` has a row here and in the manual, and that the manual's types
//! and defaults match the code.

use crate::session::Settings;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Category {
    Appearance,
    Editor,
    Files,
    Find,
    Terminal,
    SideBar,
    Help,
    Advanced,
}

impl Category {
    pub const ALL: [Category; 8] = [Category::Appearance, Category::Editor, Category::Files, Category::Find, Category::Terminal, Category::SideBar, Category::Help, Category::Advanced];

    pub fn label(self) -> &'static str {
        match self {
            Category::Appearance => "Appearance",
            Category::Editor => "Editor",
            Category::Files => "Files & Saving",
            Category::Find => "Find & Spelling",
            Category::Terminal => "Terminal & Build",
            Category::SideBar => "Side Bar",
            Category::Help => "Help",
            Category::Advanced => "Advanced",
        }
    }
}

/// How a setting is edited.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Control {
    Toggle,
    /// A number between `min` and `max`; `whole` numbers are stored as integers.
    Number { min: f64, max: f64, step: f64, whole: bool },
    /// A whole number, or null (shown as `none`).
    MaybeNumber { min: f64, max: f64, none: &'static str },
    /// One of these values, with labels.
    Choice(&'static [(&'static str, &'static str)]),
    Text,
    /// A font name (empty: the built-in font).
    Font { editor: bool },
    /// A color scheme name.
    Scheme,
    /// A list of words or globs.
    Words,
    /// A list of column numbers.
    Columns,
    /// A list holding any of these values, each a checkbox.
    Flags(&'static [(&'static str, &'static str)]),
    /// Too structured for a form: edited in settings.json.
    Json,
}

pub struct Meta {
    pub key: &'static str,
    pub category: Category,
    pub label: &'static str,
    pub control: Control,
}

const fn m(key: &'static str, category: Category, label: &'static str, control: Control) -> Meta {
    Meta { key, category, label, control }
}

const fn num(min: f64, max: f64, step: f64) -> Control {
    Control::Number { min, max, step, whole: false }
}

const fn whole(min: f64, max: f64) -> Control {
    Control::Number { min, max, step: 1.0, whole: true }
}

use Category::*;
use Control::*;

/// Every setting, in the order the window shows them.
pub static SETTINGS: &[Meta] = &[
    // Appearance
    m("theme_mode", Appearance, "Color scheme mode", Choice(&[("fixed", "Fixed"), ("auto", "Follow system"), ("dark", "Always dark"), ("light", "Always light")])),
    m("theme", Appearance, "Color scheme", Scheme),
    m("dark_theme", Appearance, "Dark mode scheme", Scheme),
    m("light_theme", Appearance, "Light mode scheme", Scheme),
    m("font_face", Appearance, "Editor font", Font { editor: true }),
    m("font_size", Appearance, "Editor font size", num(8.0, 48.0, 1.0)),
    m("line_height", Appearance, "Line height", num(0.8, 3.0, 0.05)),
    m("line_padding_top", Appearance, "Space above lines", num(0.0, 40.0, 1.0)),
    m("line_padding_bottom", Appearance, "Space below lines", num(0.0, 40.0, 1.0)),
    m("font_options", Appearance, "Font styles", Flags(&[("no_italic", "No italics"), ("no_underline", "No underlines")])),
    m("ui_font_face", Appearance, "Interface font", Font { editor: false }),
    m("ui_font_size", Appearance, "Interface font size", num(9.0, 24.0, 1.0)),
    m("caret_style", Appearance, "Cursor shape", Choice(&[("line", "Line"), ("block", "Block"), ("underline", "Underline")])),
    m("caret_blink", Appearance, "Cursor blinking", Choice(&[("blink", "Blink"), ("smooth", "Smooth"), ("solid", "Solid")])),
    m("caret_width", Appearance, "Cursor width", num(1.0, 6.0, 0.5)),
    m("window_opacity", Appearance, "Window opacity", num(0.3, 1.0, 0.05)),
    m("show_tabs", Appearance, "Show tabs", Toggle),
    m("show_status_bar", Appearance, "Show status bar", Toggle),
    m("minimap", Appearance, "Show minimap", Toggle),
    // Editor
    m("tab_size", Editor, "Tab width", whole(1.0, 16.0)),
    m("translate_tabs_to_spaces", Editor, "Indent with spaces", Toggle),
    m("detect_indentation", Editor, "Detect indentation", Toggle),
    m("auto_indent", Editor, "Auto indent", Toggle),
    m("smart_indent", Editor, "Smart indent", Toggle),
    m("auto_complete", Editor, "Word completions", Toggle),
    m("word_wrap", Editor, "Word wrap", Toggle),
    m("wrap_column", Editor, "Wrap column", MaybeNumber { min: 10.0, max: 400.0, none: "Window edge" }),
    m("wrap_width", Editor, "Wrap Paragraph width", whole(10.0, 400.0)),
    m("rulers", Editor, "Rulers", Columns),
    m("draw_white_space", Editor, "Show whitespace", Choice(&[("selection", "In selection"), ("all", "Everywhere"), ("none", "Never")])),
    m("highlight_trailing_whitespace", Editor, "Highlight trailing whitespace", Toggle),
    m("bracket_colors", Editor, "Bracket colors", Toggle),
    m("color_swatches", Editor, "Color swatches", Toggle),
    m("sticky_scroll", Editor, "Sticky scroll", Toggle),
    m("smooth_scroll", Editor, "Smooth scrolling", Toggle),
    m("git_gutter", Editor, "Git gutter", Toggle),
    m("markdown_preview", Editor, "Markdown preview", Toggle),
    m("lsp_enabled", Editor, "Language servers", Toggle),
    m("lsp_trust_all", Editor, "Start language servers without asking", Toggle),
    // Files & Saving
    m("hot_exit", Files, "Hot exit", Toggle),
    m("trim_trailing_white_space_on_save", Files, "Trim trailing whitespace on save", Toggle),
    m("ensure_newline_at_eof_on_save", Files, "Newline at end of file on save", Toggle),
    m("save_on_focus_lost", Files, "Save when Strata loses focus", Toggle),
    m("auto_save_delay_ms", Files, "Auto save delay (ms, 0 = off)", num(0.0, 60000.0, 250.0)),
    m("large_file_mb", Files, "Large-file mode above (MB)", num(1.0, 100_000.0, 1.0)),
    m("large_file_warning_mb", Files, "Ask before opening files above (MB)", num(1.0, 1_000_000.0, 10.0)),
    // Find & Spelling
    m("auto_find_in_selection", Find, "Find in selection automatically", Toggle),
    m("spell_check", Find, "Spell check", Toggle),
    m("dictionary", Find, "Dictionary", Choice(&[("en_US", "English (United States)"), ("en_GB", "English (United Kingdom)")])),
    m("added_words", Find, "Added words", Words),
    m("spelling_selector", Find, "Spell check scopes", Text),
    // Terminal & Build
    m("save_all_on_build", Terminal, "Save all before building", Toggle),
    m("terminal_visible", Terminal, "Terminal panel open", Toggle),
    m("terminal_height", Terminal, "Terminal height", num(80.0, 2000.0, 10.0)),
    m("terminal_option_as_meta", Terminal, if cfg!(target_os = "macos") { "Option key as Meta" } else { "Alt key as Meta" }, Toggle),
    m("terminal_allow_clipboard_write", Terminal, "Programs may set the clipboard", Toggle),
    m("terminal_allow_clipboard_read", Terminal, "Programs may read the clipboard", Toggle),
    // Side Bar
    m("sidebar_respect_gitignore", SideBar, "Hide ignored files", Toggle),
    m("file_exclude_patterns", SideBar, "Hidden names", Words),
    // Help
    m("help_in_own_window", Help, "Documentation in its own window", Toggle),
    m("update_check", Help, "Check for updates", Choice(&[("ask", "Ask me"), ("auto", "Automatically"), ("off", "Manually only")])),
    m("update_check_interval_hours", Help, "Check every", Choice(&[("1", "1 hour"), ("2", "2 hours"), ("4", "4 hours"), ("6", "6 hours"), ("12", "12 hours")])),
    m("update_channel", Help, "Updates to offer", Choice(&[("stable", "Releases"), ("prerelease", "Releases and prereleases")])),
    // Advanced
    m("editorconfig", Editor, "Follow .editorconfig files", Toggle),
    m("syntax_settings", Advanced, "Settings per language", Json),
    m("lsp_servers", Advanced, "Language server setup", Json),
    m("theme_overrides", Advanced, "Color overrides", Json),
    m("theme_overrides_by_theme", Advanced, "Color overrides per scheme", Json),
];

#[cfg(test)]
pub fn meta(key: &str) -> Option<&'static Meta> {
    SETTINGS.iter().find(|m| m.key == key)
}

/// Settings as JSON values by key.
pub fn values(s: &Settings) -> Map<String, Value> {
    match serde_json::to_value(s) {
        Ok(Value::Object(map)) => map,
        _ => Map::new(),
    }
}

pub fn defaults() -> &'static Map<String, Value> {
    static D: OnceLock<Map<String, Value>> = OnceLock::new();
    D.get_or_init(|| values(&Settings::default()))
}

pub fn is_default(values: &Map<String, Value>, key: &str) -> bool {
    match (values.get(key), defaults().get(key)) {
        (Some(v), Some(d)) => crate::jsonc::same(v, d),
        _ => true,
    }
}

/// Set one setting from a JSON value. An error (and no change) if the
/// value doesn't fit the setting.
pub fn set(s: &mut Settings, key: &str, value: Value) -> Result<(), String> {
    let mut map = values(s);
    if !map.contains_key(key) {
        return Err(format!("unknown setting {key}"));
    }
    map.insert(key.to_owned(), value);
    *s = serde_json::from_value(Value::Object(map)).map_err(|e| format!("{key}: {e}"))?;
    Ok(())
}

/// A problem with a value that is in the file but outside what Strata uses.
pub fn range_problem(meta: &Meta, value: &Value) -> Option<String> {
    let (min, max) = match meta.control {
        Number { min, max, .. } | MaybeNumber { min, max, .. } => (min, max),
        _ => return None,
    };
    let v = value.as_f64()?;
    (v < min || v > max).then(|| format!("{} is outside {}–{}; Strata uses the nearest allowed value", fmt_number(v), fmt_number(min), fmt_number(max)))
}

pub fn fmt_number(v: f64) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// What the manual says about a setting.
#[derive(Clone, Debug, Default)]
#[cfg_attr(not(test), allow(dead_code))]
pub struct Doc {
    /// "true/false", "number", "text", "list of text"…
    pub kind: String,
    /// The Default column, as written.
    pub default: String,
    /// The description, as plain text (links and code marks removed).
    pub text: String,
}

/// The manual's settings tables, by key.
pub fn docs() -> &'static HashMap<String, Doc> {
    static D: OnceLock<HashMap<String, Doc>> = OnceLock::new();
    D.get_or_init(|| {
        let src = crate::help::page("settings.md").map(|p| p.text()).unwrap_or_default();
        doc_rows(&src).into_iter().collect()
    })
}

/// `| `key` | type | default | description |` rows of the settings tables.
pub fn doc_rows(src: &str) -> Vec<(String, Doc)> {
    let mut out = Vec::new();
    for line in src.lines() {
        let Some(row) = line.trim().strip_prefix("| `").map(|_| &line.trim()[1..]) else { continue };
        let cells: Vec<&str> = split_cells(row);
        if cells.len() < 4 {
            continue;
        }
        let Some(key) = cells[0].trim().strip_prefix('`').and_then(|k| k.strip_suffix('`')) else { continue };
        if key.contains(' ') || key.contains('`') {
            continue;
        }
        out.push((key.to_owned(), Doc { kind: cells[1].trim().to_owned(), default: cells[2].trim().to_owned(), text: plain(cells[3].trim()) }));
    }
    out
}

/// Table cells, splitting on `|` outside code spans.
fn split_cells(row: &str) -> Vec<&str> {
    let mut cells = Vec::new();
    let (mut start, mut code) = (0, false);
    for (i, c) in row.char_indices() {
        match c {
            '`' => code = !code,
            '|' if !code => {
                cells.push(&row[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    cells
}

/// Markdown to plain text: links keep their text, code and bold marks go.
pub fn plain(md: &str) -> String {
    let mut out = String::with_capacity(md.len());
    let mut rest = md;
    while let Some(open) = rest.find('[') {
        let Some(close) = rest[open..].find("](").map(|c| open + c) else { break };
        let Some(end) = rest[close..].find(')').map(|e| close + e) else { break };
        out.push_str(&rest[..open]);
        out.push_str(&rest[open + 1..close]);
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    // Emphasis marks go; code spans lose their backticks but keep a `*` inside.
    let mut code = false;
    out.chars()
        .filter(|&c| {
            if c == '`' {
                code = !code;
            }
            c != '`' && (code || c != '*')
        })
        .collect()
}

/// The manual's description of `key`, as plain text.
pub fn description(key: &str) -> String {
    docs().get(key).map(|d| d.text.clone()).unwrap_or_default()
}

/// Does `meta` match a search? Every word must appear in its label, key,
/// category or description (case doesn't matter).
pub fn matches(meta: &Meta, query: &str) -> bool {
    let hay = format!("{} {} {} {}", meta.label, meta.key, meta.category.label(), description(meta.key)).to_lowercase();
    query.split_whitespace().all(|w| hay.contains(&w.to_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw_docs() -> HashMap<String, Doc> {
        doc_rows(crate::help::page("settings.md").unwrap().text().as_str()).into_iter().collect()
    }

    #[test]
    fn every_setting_has_one_row_here_and_in_the_manual() {
        let fields = values(&Settings::default());
        let docs = raw_docs();
        for key in fields.keys() {
            assert!(meta(key).is_some(), "settings_meta::SETTINGS has no row for `{key}`");
            assert!(docs.contains_key(key), "settings.md's tables have no row for `{key}`");
        }
        let mut seen = Vec::new();
        for m in SETTINGS {
            assert!(fields.contains_key(m.key), "SETTINGS lists `{}`, which Settings doesn't have", m.key);
            assert!(!seen.contains(&m.key), "`{}` listed twice", m.key);
            seen.push(m.key);
            assert!(!description(m.key).is_empty(), "{}", m.key);
        }
        for key in docs.keys() {
            assert!(fields.contains_key(key), "settings.md documents `{key}`, which Settings doesn't have");
        }
    }

    #[test]
    fn manual_types_and_defaults_match_the_code() {
        let docs = raw_docs();
        for m in SETTINGS {
            let doc = &docs[m.key];
            let kind = doc.kind.as_str();
            let ok = match m.control {
                Toggle => kind == "true/false",
                Number { whole: true, .. } => kind == "whole number",
                Number { .. } => kind == "number",
                MaybeNumber { .. } => kind.starts_with("number or"),
                // A list of numbers (hours) keeps its number type.
                Choice(_) => kind == "text" || kind == "whole number",
                Text | Font { .. } | Scheme => kind == "text",
                Words | Flags(_) => kind == "list of text",
                Columns => kind == "list of numbers",
                Json => kind == "object",
            };
            assert!(ok, "`{}`: the manual says {kind}, the window edits it as {:?}", m.key, m.control);
            // Defaults written as JSON in the manual must be the real ones.
            let cell = doc.default.trim_matches('`');
            if let Ok(v) = serde_json::from_str::<Value>(cell) {
                assert!(crate::jsonc::same(&v, &defaults()[m.key]), "`{}`: the manual's default {cell} isn't {}", m.key, defaults()[m.key]);
            } else {
                assert!(matches!(m.key, "spelling_selector" | "terminal_option_as_meta"), "`{}`: default {cell:?} isn't JSON", m.key);
            }
            // Choices offered are the values the manual names.
            if let Choice(options) = m.control {
                for (value, _) in options {
                    assert!(doc.text.contains(&format!("\"{value}\"")) || [format!("{value},"), format!(" {value} "), format!(" {value}.")].iter().any(|w| doc.text.contains(w.as_str())) || doc.default.contains(value), "`{}`: the manual doesn't mention {value:?}", m.key);
                }
            }
            if let Number { min, max, .. } = m.control {
                assert!(range_problem(m, &defaults()[m.key]).is_none(), "`{}`'s default is outside {min}–{max}", m.key);
            }
        }
    }

    #[test]
    fn set_and_reset_by_key() {
        let mut s = Settings::default();
        set(&mut s, "font_size", Value::from(18.0)).unwrap();
        assert_eq!(s.font_size, 18.0);
        set(&mut s, "rulers", serde_json::json!([80, 100])).unwrap();
        assert_eq!(s.rulers, [80, 100]);
        assert!(set(&mut s, "tab_size", Value::from("wide")).is_err());
        assert_eq!(s.tab_size, 4, "a bad value changes nothing");
        assert!(set(&mut s, "no_such_setting", Value::Bool(true)).is_err());
        let v = values(&s);
        assert!(!is_default(&v, "font_size") && is_default(&v, "tab_size"));
        set(&mut s, "font_size", defaults()["font_size"].clone()).unwrap();
        assert_eq!(s.font_size, 14.0);
    }

    #[test]
    fn search_looks_at_labels_keys_and_descriptions() {
        let hits = |q: &str| SETTINGS.iter().filter(|m| matches(m, q)).map(|m| m.key).collect::<Vec<_>>();
        assert!(hits("font").contains(&"font_face") && hits("font").contains(&"ui_font_size"));
        assert_eq!(hits("trim_trailing"), ["trim_trailing_white_space_on_save"]);
        assert!(hits("OSC 52").contains(&"terminal_allow_clipboard_read"), "descriptions count");
        assert!(hits("gitignore").contains(&"sidebar_respect_gitignore"));
        assert!(hits("zzqq").is_empty());
    }

    #[test]
    fn markdown_becomes_plain_text() {
        assert_eq!(plain("See [Fonts](appearance.md#fonts) and `x`, **⌘ ,**."), "See Fonts and x, ⌘ ,.");
        assert_eq!(plain("Wrap (*no shortcut*), for example `[\"*.pyc\"]`."), "Wrap (no shortcut), for example [\"*.pyc\"].");
        assert_eq!(fmt_number(1.0), "1");
        assert_eq!(fmt_number(0.35), "0.35");
        let rows = doc_rows("| Setting | Type |\n| `a` | text | `\"x|y\"` | Hi [b](c.md). |\n");
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].0.as_str(), rows[0].1.default.as_str(), rows[0].1.text.as_str()), ("a", "`\"x|y\"`", "Hi b."));
    }
}
