//! Color scheme customization: user color overrides from settings, Sublime
//! Text 3/4 `.sublime-color-scheme` files, and choosing a scheme by the
//! system's light or dark appearance.

use super::themes::{Kind, RULES, Role, luminance};
use super::{Scheme, rgb_color};
use egui::Color32;
use std::collections::BTreeMap;
use std::str::FromStr;
use syntect::highlighting::{Color, FontStyle, ScopeSelectors, StyleModifier, Theme, ThemeItem, ThemeSettings, UnderlineOption};
use syntect::parsing::Scope;

// ---- colors -------------------------------------------------------------------------

/// Parse a CSS-style color as Sublime's color schemes write them: `#rgb`,
/// `#rgba`, `#rrggbb`, `#rrggbbaa`, `rgb()`/`rgba()`, `hsl()`/`hsla()`, a few
/// names, `var(name)`, and `color(<color> alpha(0.5) blend(#fff 20%) …)`.
pub fn parse_color(s: &str, vars: &BTreeMap<String, String>) -> Result<Color32, String> {
    parse_depth(s.trim(), vars, 0)
}

fn parse_depth(s: &str, vars: &BTreeMap<String, String>, depth: usize) -> Result<Color32, String> {
    if depth > 16 {
        return Err(format!("variables nest too deep at \"{s}\""));
    }
    let lower = s.to_ascii_lowercase();
    if let Some(hex) = lower.strip_prefix('#') {
        return parse_hex(hex).ok_or_else(|| format!("bad hex color \"{s}\""));
    }
    if let Some(name) = lower.strip_prefix("var(").and_then(|r| r.strip_suffix(')')) {
        let name = name.trim();
        let value = vars.get(name).ok_or_else(|| format!("unknown variable \"{name}\""))?;
        return parse_depth(value.trim(), vars, depth + 1);
    }
    if let Some(inner) = lower.strip_prefix("color(").and_then(|r| r.strip_suffix(')')) {
        return color_mod(inner, vars, depth);
    }
    for (prefix, hsl) in [("rgba(", false), ("rgb(", false), ("hsla(", true), ("hsl(", true)] {
        if let Some(args) = lower.strip_prefix(prefix).and_then(|r| r.strip_suffix(')')) {
            let parts: Vec<&str> = args.split([',', ' ', '/']).map(str::trim).filter(|p| !p.is_empty()).collect();
            if parts.len() < 3 {
                return Err(format!("bad color \"{s}\""));
            }
            let num = |p: &str| p.trim_end_matches(['%', '°']).trim_end_matches("deg").parse::<f32>().map_err(|_| format!("bad number \"{p}\" in \"{s}\""));
            let alpha = match parts.get(3) {
                Some(a) if a.ends_with('%') => num(a)? / 100.0,
                Some(a) => num(a)?,
                None => 1.0,
            };
            let a = (alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
            let (r, g, b) = if hsl {
                hsl_to_rgb(num(parts[0])?, num(parts[1])? / 100.0, num(parts[2])? / 100.0)
            } else {
                let ch = |p: &str| -> Result<u8, String> {
                    let v = num(p)?;
                    Ok(if p.ends_with('%') { v * 2.55 } else { v }.round().clamp(0.0, 255.0) as u8)
                };
                (ch(parts[0])?, ch(parts[1])?, ch(parts[2])?)
            };
            return Ok(Color32::from_rgba_unmultiplied(r, g, b, a));
        }
    }
    named(&lower).ok_or_else(|| format!("unknown color \"{s}\""))
}

fn parse_hex(h: &str) -> Option<Color32> {
    if !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let digit = |i: usize| u8::from_str_radix(&h[i..=i], 16).ok().map(|v| v * 17);
    let pair = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    let (r, g, b, a) = match h.len() {
        3 => (digit(0)?, digit(1)?, digit(2)?, 255),
        4 => (digit(0)?, digit(1)?, digit(2)?, digit(3)?),
        6 => (pair(0)?, pair(2)?, pair(4)?, 255),
        8 => (pair(0)?, pair(2)?, pair(4)?, pair(6)?),
        _ => return None,
    };
    Some(Color32::from_rgba_unmultiplied(r, g, b, a))
}

fn named(name: &str) -> Option<Color32> {
    Some(match name {
        "black" => Color32::BLACK,
        "white" => Color32::WHITE,
        "transparent" => Color32::from_rgba_unmultiplied(0, 0, 0, 0),
        "red" => Color32::from_rgb(255, 0, 0),
        "green" => Color32::from_rgb(0, 128, 0),
        "blue" => Color32::from_rgb(0, 0, 255),
        "yellow" => Color32::from_rgb(255, 255, 0),
        "orange" => Color32::from_rgb(255, 165, 0),
        "purple" => Color32::from_rgb(128, 0, 128),
        "gray" | "grey" => Color32::from_rgb(128, 128, 128),
        "silver" => Color32::from_rgb(192, 192, 192),
        "cyan" | "aqua" => Color32::from_rgb(0, 255, 255),
        "magenta" | "fuchsia" => Color32::from_rgb(255, 0, 255),
        "pink" => Color32::from_rgb(255, 192, 203),
        _ => return None,
    })
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
    let h = h.rem_euclid(360.0) / 360.0;
    let (s, l) = (s.clamp(0.0, 1.0), l.clamp(0.0, 1.0));
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let hue = |mut t: f32| {
        t = t.rem_euclid(1.0);
        let v = if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        };
        (v * 255.0).round().clamp(0.0, 255.0) as u8
    };
    (hue(h + 1.0 / 3.0), hue(h), hue(h - 1.0 / 3.0))
}

fn rgb_to_hsl(c: Color32) -> (f32, f32, f32) {
    let (r, g, b) = (c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (max + min) / 2.0;
    if max == min {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h * 60.0, s, l)
}

/// Split at top-level spaces (not inside parentheses).
fn split_top(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut start) = (0i32, 0);
    for (i, ch) in s.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            ' ' | '\t' if depth == 0 => {
                if i > start {
                    out.push(&s[start..i]);
                }
                start = i + 1;
            }
            _ => {}
        }
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// Sublime's `color()` mod: a base color and adjusters applied in order.
fn color_mod(inner: &str, vars: &BTreeMap<String, String>, depth: usize) -> Result<Color32, String> {
    let parts = split_top(inner.trim());
    let (base, adjusters) = parts.split_first().ok_or("empty color()")?;
    let mut c = parse_depth(base, vars, depth + 1)?;
    for adj in adjusters {
        let (func, arg) = adj.split_once('(').and_then(|(f, rest)| rest.strip_suffix(')').map(|a| (f.trim(), a.trim()))).ok_or_else(|| format!("bad adjuster \"{adj}\""))?;
        let amount = |a: &str| -> Result<(f32, bool), String> {
            let rel = a.starts_with('+') || a.starts_with('-');
            let v: f32 = a.trim_end_matches('%').parse().map_err(|_| format!("bad amount \"{a}\""))?;
            Ok((if a.ends_with('%') || v.abs() > 1.0 { v / 100.0 } else { v }, rel))
        };
        c = match func {
            "alpha" | "a" => {
                let (v, rel) = amount(arg)?;
                let a = if rel { c.a() as f32 / 255.0 + v } else { v };
                Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (a.clamp(0.0, 1.0) * 255.0).round() as u8)
            }
            "blend" | "blenda" => {
                let args = split_top(arg);
                let (other, pct) = match args.as_slice() {
                    [other, pct, ..] => (parse_depth(other, vars, depth + 1)?, amount(pct)?.0),
                    _ => return Err(format!("bad blend \"{adj}\"")),
                };
                // `pct` is how much of the base color to keep.
                let m = |x: u8, y: u8| (x as f32 * pct + y as f32 * (1.0 - pct)).round() as u8;
                let a = if func == "blenda" { m(c.a(), other.a()) } else { c.a() };
                Color32::from_rgba_unmultiplied(m(c.r(), other.r()), m(c.g(), other.g()), m(c.b(), other.b()), a)
            }
            "lightness" | "l" | "saturation" | "s" => {
                let (v, rel) = amount(arg)?;
                let (h, s, l) = rgb_to_hsl(c);
                let (s, l) = if func.starts_with('l') { (s, if rel { l + v } else { v }) } else { (if rel { s + v } else { v }, l) };
                let (r, g, b) = hsl_to_rgb(h, s, l);
                Color32::from_rgba_unmultiplied(r, g, b, c.a())
            }
            "min-contrast" => c,
            other => return Err(format!("unsupported color adjuster \"{other}\"")),
        };
    }
    Ok(c)
}

fn syn(c: Color32) -> Color {
    rgb_color(c)
}

// ---- .sublime-color-scheme import ----------------------------------------------------

/// Convert a Sublime Text 3/4 `.sublime-color-scheme` (JSON, comments and
/// trailing commas allowed) into a scheme. `fallback_name` names it when the
/// file doesn't. Problems with single rules are returned as warnings and the
/// rule is skipped; a file that isn't JSON at all is an error.
pub fn import_sublime_color_scheme(text: &str, fallback_name: &str) -> Result<(Scheme, Vec<String>), String> {
    let json: serde_json::Value = serde_json::from_str(&crate::session::strip_jsonc(text)).map_err(|e| e.to_string())?;
    let obj = json.as_object().ok_or("not a JSON object")?;
    let warnings = std::cell::RefCell::new(Vec::new());
    let vars: BTreeMap<String, String> = obj
        .get("variables")
        .and_then(|v| v.as_object())
        .map(|m| m.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_owned()))).collect())
        .unwrap_or_default();
    let color = |value: &serde_json::Value, what: &str| -> Option<Color32> {
        // Hashed foregrounds are arrays; use the first color.
        let s = value.as_str().or_else(|| value.as_array().and_then(|a| a.first()).and_then(|v| v.as_str()))?;
        match parse_color(s, &vars) {
            Ok(c) => Some(c),
            Err(e) => {
                warnings.borrow_mut().push(format!("{what}: {e}"));
                None
            }
        }
    };

    let mut settings = ThemeSettings::default();
    let mut whitespace = None;
    if let Some(globals) = obj.get("globals").and_then(|g| g.as_object()) {
        for (key, value) in globals {
            if matches!(key.as_str(), "popup_css" | "phantom_css" | "sheet_css") {
                let css = value.as_str().map(str::to_owned);
                match key.as_str() {
                    "popup_css" => settings.popup_css = css,
                    "phantom_css" => settings.phantom_css = css,
                    _ => {}
                }
                continue;
            }
            if key.ends_with("_options") {
                let opt = value.as_str().and_then(|s| UnderlineOption::from_str(s.split_whitespace().next().unwrap_or("")).ok());
                match key.as_str() {
                    "brackets_options" => settings.brackets_options = opt,
                    "bracket_contents_options" => settings.bracket_contents_options = opt,
                    "tags_options" => settings.tags_options = opt,
                    _ => {}
                }
                continue;
            }
            let Some(c) = color(value, &format!("globals.{key}")) else { continue };
            let slot = match key.as_str() {
                "background" => &mut settings.background,
                "foreground" => &mut settings.foreground,
                "caret" => &mut settings.caret,
                "line_highlight" => &mut settings.line_highlight,
                "selection" => &mut settings.selection,
                "selection_foreground" => &mut settings.selection_foreground,
                "selection_border" => &mut settings.selection_border,
                "inactive_selection" => &mut settings.inactive_selection,
                "inactive_selection_foreground" => &mut settings.inactive_selection_foreground,
                "gutter" => &mut settings.gutter,
                "gutter_foreground" => &mut settings.gutter_foreground,
                "find_highlight" => &mut settings.find_highlight,
                "find_highlight_foreground" => &mut settings.find_highlight_foreground,
                "guide" => &mut settings.guide,
                "active_guide" => &mut settings.active_guide,
                "stack_guide" => &mut settings.stack_guide,
                "brackets_foreground" => &mut settings.brackets_foreground,
                "brackets_background" => &mut settings.brackets_background,
                "bracket_contents_foreground" => &mut settings.bracket_contents_foreground,
                "tags_foreground" => &mut settings.tags_foreground,
                "highlight" => &mut settings.highlight,
                "misspelling" => &mut settings.misspelling,
                "shadow" => &mut settings.shadow,
                "accent" => &mut settings.accent,
                "minimap_border" => &mut settings.minimap_border,
                "invisibles" | "whitespace" => {
                    whitespace = Some(syn(c));
                    continue;
                }
                // Other globals (diff gutter colors, rulers, …) Strata doesn't draw.
                _ => continue,
            };
            *slot = Some(syn(c));
        }
    }

    let mut scopes = Vec::new();
    for (i, rule) in obj.get("rules").and_then(|r| r.as_array()).into_iter().flatten().enumerate() {
        let Some(rule) = rule.as_object() else { continue };
        let label = rule.get("name").and_then(|n| n.as_str()).map_or_else(|| format!("rules[{i}]"), |n| format!("rule \"{n}\""));
        let Some(scope) = rule.get("scope").and_then(|s| s.as_str()) else {
            warnings.borrow_mut().push(format!("{label}: no scope"));
            continue;
        };
        let Ok(selectors) = ScopeSelectors::from_str(scope) else {
            warnings.borrow_mut().push(format!("{label}: bad scope \"{scope}\""));
            continue;
        };
        let foreground = rule.get("foreground").and_then(|v| color(v, &label)).map(syn);
        let background = rule.get("background").and_then(|v| color(v, &label)).map(syn);
        let font_style = rule.get("font_style").and_then(|v| v.as_str()).map(|s| {
            let mut f = FontStyle::empty();
            for word in s.split_whitespace() {
                match word {
                    "bold" => f |= FontStyle::BOLD,
                    "italic" => f |= FontStyle::ITALIC,
                    "underline" | "stippled_underline" | "squiggly_underline" => f |= FontStyle::UNDERLINE,
                    _ => {}
                }
            }
            f
        });
        scopes.push(ThemeItem { scope: selectors, style: StyleModifier { foreground, background, font_style } });
    }
    if settings.background.is_none() {
        warnings.borrow_mut().push("globals.background missing".into());
    }
    let name = obj.get("name").and_then(|n| n.as_str()).filter(|n| !n.trim().is_empty()).unwrap_or(fallback_name).to_owned();
    let author = obj.get("author").and_then(|n| n.as_str()).map(str::to_owned);
    let theme = Theme { name: Some(name), author, settings, scopes };
    let kind = kind_of(&theme);
    Ok((Scheme { theme, kind, whitespace, ansi: None }, warnings.into_inner()))
}

/// Light or dark, judged by the background.
pub fn kind_of(theme: &Theme) -> Kind {
    let bg = theme.settings.background.map_or(Color32::BLACK, |c| Color32::from_rgb(c.r, c.g, c.b));
    if luminance(bg) > 0.3 { Kind::Light } else { Kind::Dark }
}

// ---- overrides ------------------------------------------------------------------

/// Editor colors users can override, besides the syntax roles.
pub const UI_KEYS: &[&str] = &[
    "background",
    "foreground",
    "caret",
    "selection",
    "line_highlight",
    "gutter",
    "gutter_foreground",
    "find_highlight",
    "guide",
    "bracket",
    "whitespace",
    "accent",
];

/// Customizations layered on top of every scheme (and per scheme).
#[derive(Clone, Default, PartialEq, Debug)]
pub struct Adjustments {
    pub overrides: BTreeMap<String, String>,
    pub by_theme: BTreeMap<String, BTreeMap<String, String>>,
    /// Drop italics from syntax styles (`font_options: ["no_italic"]`).
    pub no_italic: bool,
    pub no_underline: bool,
}

impl Adjustments {
    pub fn is_empty(&self) -> bool {
        self.overrides.is_empty() && self.by_theme.values().all(|m| m.is_empty()) && !self.no_italic && !self.no_underline
    }

    /// Every problem in the overrides, for the status bar.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut check = |map: &BTreeMap<String, String>, label: &str| {
            for (k, v) in map {
                let key = normalize_key(k);
                if !UI_KEYS.contains(&key.as_str()) && Role::from_key(&key).is_none() {
                    out.push(format!("{label}: unknown color \"{k}\""));
                } else if let Err(e) = parse_color(v, &BTreeMap::new()) {
                    out.push(format!("{label}.{k}: {e}"));
                }
            }
        };
        check(&self.overrides, "theme_overrides");
        for (theme, map) in &self.by_theme {
            check(map, &format!("theme_overrides_by_theme[{theme}]"));
        }
        out
    }
}

fn normalize_key(k: &str) -> String {
    let k = k.trim().to_ascii_lowercase().replace(['-', ' '], "_");
    match k.as_str() {
        "line_number" | "line_numbers" | "gutter_fg" => "gutter_foreground".into(),
        "indent_guide" | "indent_guides" | "guides" => "guide".into(),
        "brackets" | "brackets_foreground" | "bracket_match" => "bracket".into(),
        "invisibles" => "whitespace".into(),
        "current_line" | "line" => "line_highlight".into(),
        "find" | "find_match" => "find_highlight".into(),
        "cursor" => "caret".into(),
        "bg" => "background".into(),
        "fg" | "text" => "foreground".into(),
        _ => k,
    }
}

/// The role a theme rule's selector belongs to: the role whose selector is
/// the longest prefix of the rule's last scope (`comment.line` → comment,
/// `punctuation.definition.string` → string, `keyword.operator` → operator).
fn role_of(selector: &syntect::highlighting::ScopeSelector) -> Option<Role> {
    let path = selector.path.as_slice();
    let last = *path.last()?;
    let mut best: Option<(usize, Role)> = None;
    for (role, sels) in RULES {
        for scope in sels.split(',').map(str::trim) {
            // Multi-scope selectors ("meta.mapping.key string") only match themselves.
            if scope.contains(' ') {
                if path.len() > 1 && path.iter().map(|s| s.build_string()).collect::<Vec<_>>().join(" ") == scope {
                    return Some(*role);
                }
                continue;
            }
            let Ok(s) = Scope::new(scope) else { continue };
            if s.is_prefix_of(last) && best.is_none_or(|(len, _)| s.len() as usize > len) {
                best = Some((s.len() as usize, *role));
            }
        }
    }
    best.map(|(_, r)| r)
}

/// Apply one override map to `scheme`. Unknown keys and bad colors are
/// skipped (see `Adjustments::problems`).
pub fn apply_overrides(scheme: &mut Scheme, map: &BTreeMap<String, String>) {
    let vars = BTreeMap::new();
    for (k, v) in map {
        let Ok(color) = parse_color(v, &vars) else { continue };
        let c = syn(color);
        let s = &mut scheme.theme.settings;
        match normalize_key(k).as_str() {
            "background" => s.background = Some(c),
            "foreground" => s.foreground = Some(c),
            "caret" => s.caret = Some(c),
            "selection" => s.selection = Some(c),
            "line_highlight" => s.line_highlight = Some(c),
            "gutter" => s.gutter = Some(c),
            "gutter_foreground" => s.gutter_foreground = Some(c),
            "find_highlight" => s.find_highlight = Some(c),
            "guide" => s.guide = Some(c),
            "bracket" => s.brackets_foreground = Some(c),
            "whitespace" => scheme.whitespace = Some(c),
            "accent" => s.accent = Some(c),
            key => {
                let Some(role) = Role::from_key(key) else { continue };
                // Recolor the scheme's own rules for this role, then add a
                // catch-all in front (on ties syntect keeps the first rule).
                for item in &mut scheme.theme.scopes {
                    if item.scope.selectors.iter().any(|sel| role_of(sel) == Some(role)) {
                        item.style.foreground = Some(c);
                    }
                }
                let selectors = RULES.iter().find(|r| r.0 == role).map(|r| r.1).unwrap_or_default();
                if let Ok(scope) = ScopeSelectors::from_str(selectors) {
                    scheme.theme.scopes.insert(0, ThemeItem { scope, style: StyleModifier { foreground: Some(c), background: None, font_style: None } });
                }
            }
        }
    }
    if map.keys().any(|k| normalize_key(k) == "background") {
        scheme.kind = match (scheme.kind.is_high_contrast(), kind_of(&scheme.theme).is_light()) {
            (true, true) => Kind::HighContrastLight,
            (true, false) => Kind::HighContrastDark,
            (false, true) => Kind::Light,
            (false, false) => Kind::Dark,
        };
    }
}

/// `scheme` named `name` with every adjustment that applies to it.
pub fn adjusted(scheme: &Scheme, name: &str, adj: &Adjustments) -> Scheme {
    let mut out = scheme.clone();
    if adj.no_italic || adj.no_underline {
        for item in &mut out.theme.scopes {
            if let Some(f) = &mut item.style.font_style {
                if adj.no_italic {
                    f.remove(FontStyle::ITALIC);
                }
                if adj.no_underline {
                    f.remove(FontStyle::UNDERLINE);
                }
            }
        }
    }
    apply_overrides(&mut out, &adj.overrides);
    if let Some(map) = adj.by_theme.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v) {
        apply_overrides(&mut out, map);
    }
    out
}

// ---- following the system appearance --------------------------------------------------

/// The scheme to show: `theme_mode` "auto" (or `"theme": "auto"`, as in
/// Sublime) follows the system, using `light_theme` in light mode and
/// `dark_theme` otherwise; "dark"/"light" pin one of the pair; anything else
/// uses `theme`.
pub fn choose<'a>(mode: &str, theme: &'a str, dark: &'a str, light: &'a str, system_light: Option<bool>) -> &'a str {
    match mode.trim().to_ascii_lowercase().as_str() {
        "dark" => dark,
        "light" => light,
        "auto" | "system" => {
            if system_light == Some(true) {
                light
            } else {
                dark
            }
        }
        _ if theme.eq_ignore_ascii_case("auto") => choose("auto", theme, dark, light, system_light),
        _ => theme,
    }
}

/// Where choosing scheme `name` (light or not) is stored, given the mode:
/// in auto mode it becomes the light or dark scheme by its own kind.
#[derive(Debug, PartialEq, Eq)]
pub enum Slot {
    Theme,
    Dark,
    Light,
}

pub fn slot_for(mode: &str, picked_is_light: bool) -> Slot {
    match (mode.trim().to_ascii_lowercase().as_str(), picked_is_light) {
        ("auto" | "system", true) | ("light", true) => Slot::Light,
        ("auto" | "system", false) | ("dark", false) => Slot::Dark,
        _ => Slot::Theme,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(s: &str) -> Color32 {
        parse_color(s, &BTreeMap::new()).unwrap()
    }

    #[test]
    fn colors_parse_in_every_css_form() {
        assert_eq!(c("#abc"), Color32::from_rgb(0xaa, 0xbb, 0xcc));
        assert_eq!(c("#1e1e2e"), Color32::from_rgb(0x1e, 0x1e, 0x2e));
        assert_eq!(c("#1e1e2e80").a(), 0x80);
        assert_eq!(c("rgb(255, 0, 10)"), Color32::from_rgb(255, 0, 10));
        assert_eq!(c("rgba(255, 0, 10, 0.5)").a(), 128);
        assert_eq!(c("hsl(0, 100%, 50%)"), Color32::from_rgb(255, 0, 0));
        assert_eq!(c("hsla(120, 100%, 25%, 1)"), Color32::from_rgb(0, 128, 0));
        assert_eq!(c("white"), Color32::WHITE);
        assert!(parse_color("#12", &BTreeMap::new()).is_err());
        assert!(parse_color("bogus", &BTreeMap::new()).is_err());
        let vars = BTreeMap::from([("blue".to_owned(), "#0000ff".to_owned()), ("accent".to_owned(), "var(blue)".to_owned())]);
        assert_eq!(parse_color("var(accent)", &vars).unwrap(), Color32::from_rgb(0, 0, 255));
        assert_eq!(parse_color("color(var(blue) alpha(0.25))", &vars).unwrap().a(), 64);
        let half = parse_color("color(#000000 blend(#ffffff 50%))", &vars).unwrap();
        assert!((126..=129).contains(&half.r()), "{half:?}");
        let loop_vars = BTreeMap::from([("a".to_owned(), "var(a)".to_owned())]);
        assert!(parse_color("var(a)", &loop_vars).is_err(), "self-reference terminates");
    }

    const SCHEME: &str = r##"{
        // A comment, and a trailing comma below.
        "name": "Test Scheme",
        "variables": { "bg": "#101010", "red": "hsl(0, 100%, 60%)", "soft": "color(var(red) alpha(0.5))", },
        "globals": {
            "background": "var(bg)", "foreground": "#eeeeee", "caret": "var(red)",
            "selection": "var(soft)", "line_highlight": "#202020", "gutter_foreground": "#777",
            "brackets_options": "underline", "invisibles": "#333333",
        },
        "rules": [
            { "name": "Comment", "scope": "comment", "foreground": "#888888", "font_style": "italic" },
            { "scope": "keyword, storage", "foreground": "var(red)", "font_style": "bold italic" },
            { "scope": "string", "foreground": ["#00ff00", "#00aa00"] },
            { "scope": "markup.inserted", "background": "#003300" },
            { "scope": "constant", "foreground": "var(nope)" },
            { "foreground": "#fff" },
        ],
    }"##;

    #[test]
    fn sublime_color_schemes_import() {
        let (scheme, warnings) = import_sublime_color_scheme(SCHEME, "file").unwrap();
        let t = &scheme.theme;
        assert_eq!(t.name.as_deref(), Some("Test Scheme"));
        assert_eq!(t.settings.background, Some(Color { r: 0x10, g: 0x10, b: 0x10, a: 255 }));
        assert_eq!(t.settings.caret, Some(Color { r: 255, g: 51, b: 51, a: 255 }));
        assert_eq!(t.settings.selection.map(|c| c.a), Some(128));
        assert_eq!(t.settings.brackets_options, Some(UnderlineOption::Underline));
        assert!(scheme.whitespace.is_some());
        assert_eq!(scheme.kind, Kind::Dark);
        // Five rules have scopes; the one with an unknown variable keeps its scope but no color.
        assert_eq!(t.scopes.len(), 5);
        assert!(t.scopes[0].style.font_style.unwrap().contains(FontStyle::ITALIC));
        assert_eq!(t.scopes[1].style.font_style, Some(FontStyle::BOLD | FontStyle::ITALIC));
        assert_eq!(t.scopes[2].style.foreground, Some(Color { r: 0, g: 255, b: 0, a: 255 }), "hashed colors use the first");
        assert!(t.scopes[3].style.background.is_some());
        assert!(warnings.iter().any(|w| w.contains("nope")), "{warnings:?}");
        assert!(warnings.iter().any(|w| w.contains("no scope")), "{warnings:?}");
        assert!(import_sublime_color_scheme("not json", "x").is_err());
        let (unnamed, _) = import_sublime_color_scheme(r##"{"globals": {"background": "#ffffff"}}"##, "Mine").unwrap();
        assert_eq!(unnamed.theme.name.as_deref(), Some("Mine"));
        assert_eq!(unnamed.kind, Kind::Light);
    }

    #[test]
    fn overrides_recolor_ui_and_syntax_roles() {
        let spec = super::super::themes::SPECS.iter().find(|s| s.name == "Nord").unwrap();
        let base = super::super::themes::build(spec);
        let adj = Adjustments {
            overrides: BTreeMap::from([
                ("background".to_owned(), "#1e1e2e".to_owned()),
                ("caret".to_owned(), "#f5e0dc".to_owned()),
                ("comment".to_owned(), "#7f849c".to_owned()),
                ("Line Highlight".to_owned(), "#222233".to_owned()),
                ("whitespace".to_owned(), "#444444".to_owned()),
            ]),
            by_theme: BTreeMap::from([("nord".to_owned(), BTreeMap::from([("keyword".to_owned(), "#ff0000".to_owned())])), ("Other".to_owned(), BTreeMap::from([("string".to_owned(), "#00ff00".to_owned())]))]),
            no_italic: true,
            no_underline: false,
        };
        let s = adjusted(&base, "Nord", &adj);
        let st = &s.theme.settings;
        assert_eq!(st.background, Some(Color { r: 0x1e, g: 0x1e, b: 0x2e, a: 255 }));
        assert_eq!(st.caret, Some(Color { r: 0xf5, g: 0xe0, b: 0xdc, a: 255 }));
        assert_eq!(st.line_highlight, Some(Color { r: 0x22, g: 0x22, b: 0x33, a: 255 }));
        assert_eq!(s.whitespace, Some(Color { r: 0x44, g: 0x44, b: 0x44, a: 255 }));
        let style = |scope: &str| {
            let hl = syntect::highlighting::Highlighter::new(&s.theme);
            let stack = syntect::parsing::ScopeStack::from_str(scope).unwrap();
            hl.style_for_stack(stack.as_slice())
        };
        let rgb = |c: syntect::highlighting::Color| (c.r, c.g, c.b);
        assert_eq!(rgb(style("source.rust comment.line.double-slash.rust").foreground), (0x7f, 0x84, 0x9c));
        assert_eq!(rgb(style("source.rust punctuation.definition.comment.rust").foreground), (0x7f, 0x84, 0x9c));
        assert_eq!(rgb(style("source.rust keyword.control.rust").foreground), (0xff, 0, 0), "per-theme override");
        assert_ne!(rgb(style("source.rust keyword.operator.rust").foreground), (0xff, 0, 0), "operators keep their color");
        assert_ne!(rgb(style("source.rust string.quoted.double.rust").foreground), (0, 0xff, 0), "other theme's overrides don't apply");
        assert!(!style("source.rust comment.line.rust").font_style.contains(FontStyle::ITALIC), "no_italic");
        // The base scheme is untouched.
        assert_ne!(base.theme.settings.background, st.background);
        assert!(adj.problems().is_empty(), "{:?}", adj.problems());
        let bad = Adjustments { overrides: BTreeMap::from([("colour".to_owned(), "#fff".to_owned()), ("caret".to_owned(), "nope".to_owned())]), ..Default::default() };
        assert_eq!(bad.problems().len(), 2);
    }

    #[test]
    fn a_light_background_override_makes_the_scheme_light() {
        let spec = super::super::themes::SPECS.iter().find(|s| s.name == "One Dark").unwrap();
        let mut s = super::super::themes::build(spec);
        apply_overrides(&mut s, &BTreeMap::from([("background".to_owned(), "#fafafa".to_owned())]));
        assert_eq!(s.kind, Kind::Light);
    }

    #[test]
    fn auto_mode_follows_the_system() {
        let pick = |mode, theme, sys| choose(mode, theme, "Night", "Day", sys);
        assert_eq!(pick("auto", "X", Some(true)), "Day");
        assert_eq!(pick("auto", "X", Some(false)), "Night");
        assert_eq!(pick("auto", "X", None), "Night", "unknown appearance counts as dark");
        assert_eq!(pick("AUTO", "X", Some(true)), "Day");
        assert_eq!(pick("", "auto", Some(true)), "Day", "Sublime's \"theme\": \"auto\"");
        assert_eq!(pick("dark", "X", Some(true)), "Night");
        assert_eq!(pick("light", "X", Some(false)), "Day");
        assert_eq!(pick("fixed", "X", Some(true)), "X");
        assert_eq!(pick("", "X", None), "X");
        assert_eq!(slot_for("auto", true), Slot::Light);
        assert_eq!(slot_for("auto", false), Slot::Dark);
        assert_eq!(slot_for("dark", true), Slot::Theme);
        assert_eq!(slot_for("fixed", false), Slot::Theme);
    }
}
