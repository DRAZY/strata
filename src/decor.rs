//! Pure helpers for the editor's text decorations: trailing whitespace, CSS
//! color literals, and a bracket palette picked from the theme's own colors.

use crate::buffer::Buffer;
use crate::highlight::Highlighter;
use egui::Color32;
use std::ops::Range;

/// Char offset where the line's trailing spaces and tabs start, if it has any
/// (a line of only whitespace starts at 0).
pub fn trailing_ws(text: &str) -> Option<usize> {
    let kept = text.trim_end_matches([' ', '\t']);
    (kept.len() < text.len()).then(|| kept.chars().count())
}

/// Color literals in a line: `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`,
/// `rgb()`/`rgba()` and `hsl()`/`hsla()`, as byte ranges with their color.
pub fn find_colors(text: &str) -> Vec<(Range<usize>, Color32)> {
    let b = text.as_bytes();
    let word = |i: usize| b.get(i).is_some_and(|&c| c.is_ascii_alphanumeric() || c == b'_');
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let starts_word = i == 0 || !(word(i - 1) || b[i - 1] == b'&' || b[i - 1] == b'#');
        if c == b'#' && starts_word {
            let n = b[i + 1..].iter().take_while(|c| c.is_ascii_hexdigit()).count();
            if matches!(n, 3 | 4 | 6 | 8) && !word(i + 1 + n)
                && let Some(color) = parse_hex(&text[i + 1..i + 1 + n])
            {
                out.push((i..i + 1 + n, color));
                i += 1 + n;
                continue;
            }
        } else if (c == b'r' || c == b'h' || c == b'R' || c == b'H') && starts_word
            && let Some((len, color)) = parse_fn(&text[i..])
        {
            out.push((i..i + len, color));
            i += len;
            continue;
        }
        i += 1;
    }
    out
}

fn parse_hex(h: &str) -> Option<Color32> {
    let digit = |k: usize| u8::from_str_radix(&h[k..k + 1], 16).ok();
    let pair = |k: usize| u8::from_str_radix(&h[k..k + 2], 16).ok();
    let (r, g, b, a) = match h.len() {
        3 | 4 => {
            let d = |k| digit(k).map(|v| v * 17);
            (d(0)?, d(1)?, d(2)?, if h.len() == 4 { d(3)? } else { 255 })
        }
        6 | 8 => (pair(0)?, pair(2)?, pair(4)?, if h.len() == 8 { pair(6)? } else { 255 }),
        _ => return None,
    };
    Some(Color32::from_rgba_unmultiplied(r, g, b, a))
}

/// `rgb(…)`, `rgba(…)`, `hsl(…)` or `hsla(…)` at the start of `s`: its length and color.
fn parse_fn(s: &str) -> Option<(usize, Color32)> {
    let open = s.as_bytes().iter().take(5).position(|&c| c == b'(')?;
    let name = s[..open].to_ascii_lowercase();
    let hsl = match name.as_str() {
        "rgb" | "rgba" => false,
        "hsl" | "hsla" => true,
        _ => return None,
    };
    let close = open + s[open..].bytes().take(64).position(|c| c == b')')?;
    let args: Vec<&str> = s[open + 1..close].split([',', ' ', '/', '\t']).filter(|a| !a.is_empty()).collect();
    if !(3..=4).contains(&args.len()) {
        return None;
    }
    // A number, or a percentage of `full`.
    let num = |a: &str, full: f32| -> Option<f32> {
        match a.strip_suffix('%') {
            Some(p) => p.parse::<f32>().ok().map(|v| v / 100.0 * full),
            None => a.trim_end_matches("deg").parse::<f32>().ok(),
        }
    };
    let alpha = match args.get(3) {
        Some(a) => num(a, 1.0)?.clamp(0.0, 1.0),
        None => 1.0,
    };
    let (r, g, b) = if hsl {
        let h = num(args[0], 360.0)?.rem_euclid(360.0);
        let s = num(args[1], 100.0)?.clamp(0.0, 100.0) / 100.0;
        let l = num(args[2], 100.0)?.clamp(0.0, 100.0) / 100.0;
        hsl_to_rgb(h, s, l)
    } else {
        let c = |a: &str| num(a, 255.0).map(|v| v.clamp(0.0, 255.0) / 255.0);
        (c(args[0])?, c(args[1])?, c(args[2])?)
    };
    let byte = |v: f32| (v * 255.0).round() as u8;
    Some((close + 1, Color32::from_rgba_unmultiplied(byte(r), byte(g), byte(b), byte(alpha))))
}

/// Hue in degrees, saturation and lightness 0..1, to RGB 0..1.
fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    (r + m, g + m, b + m)
}

/// Hue in degrees, saturation and value 0..1.
fn hsv(c: Color32) -> (f32, f32, f32) {
    let (r, g, b) = (c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0);
    let max = r.max(g).max(b);
    let d = max - r.min(g).min(b);
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        60.0 * ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    (h, if max == 0.0 { 0.0 } else { d / max }, max)
}

fn luma(c: Color32) -> f32 {
    (0.299 * c.r() as f32 + 0.587 * c.g() as f32 + 0.114 * c.b() as f32) / 255.0
}

fn hue_gap(a: f32, b: f32) -> f32 {
    let d = (a - b).abs() % 360.0;
    d.min(360.0 - d)
}

/// Code that touches most scopes a theme colors.
const SAMPLE: &str = "/// Doc comment\n#[derive(Debug)]\npub struct Point<'a> { x: f64, name: &'a str }\n\
impl Point<'_> {\n    const MAX: u32 = 0xFF;\n    pub fn new(x: f64) -> Self {\n        let s = \"text\\n\";\n        \
println!(\"{}\", s.len() + 42);\n        Self { x: 1.5e3, name: 'c'.to_string().as_str() }\n    }\n}\n\
macro_rules! m { ($e:expr) => { Some(true) }; }\n";

/// 4–6 distinct, readable hues from the theme's syntax colors, ordered around
/// the color wheel from gold; fixed pleasant hues when the theme has too few.
pub fn bracket_palette(theme: &str) -> Vec<Color32> {
    let mut hl = Highlighter::new();
    hl.set_theme(theme);
    hl.set_syntax("Rust");
    let buf = Buffer::from_str(SAMPLE);
    let lines = buf.len_lines();
    let bg = hl.background();
    let mut colors: Vec<Color32> = Vec::new();
    if hl.prepare(&buf, 0..=lines - 1) {
        for l in 0..lines {
            for s in hl.line(l).iter().flat_map(|v| v.iter()) {
                let c = s.color.to_opaque();
                if !colors.contains(&c) {
                    colors.push(c);
                }
            }
        }
    }
    let readable = |c: &Color32| {
        let (_, s, _) = hsv(*c);
        s >= 0.25 && (luma(*c) - luma(bg)).abs() >= 0.22
    };
    colors.retain(readable);
    // Most vivid first, then keep each that differs enough from those kept.
    colors.sort_by(|a, b| {
        let v = |c: &Color32| {
            let (_, s, v) = hsv(*c);
            s * v
        };
        v(b).total_cmp(&v(a))
    });
    let mut picked: Vec<Color32> = Vec::new();
    for c in colors {
        if picked.len() < 6 && picked.iter().all(|p| hue_gap(hsv(*p).0, hsv(c).0) >= 30.0) {
            picked.push(c);
        }
    }
    if picked.len() < 4 {
        let light = luma(bg) > 0.55;
        picked = if light {
            [(0xB0, 0x80, 0x00), (0xA0, 0x30, 0xA0), (0x10, 0x60, 0xC0), (0x2E, 0x8B, 0x3C), (0xC0, 0x50, 0x20), (0x00, 0x88, 0x88)]
        } else {
            [(0xE8, 0xC2, 0x4A), (0xD0, 0x7C, 0xD8), (0x5C, 0xA8, 0xF0), (0x7C, 0xC8, 0x7C), (0xF0, 0x90, 0x60), (0x50, 0xC8, 0xC0)]
        }
        .map(|(r, g, b)| Color32::from_rgb(r, g, b))
        .to_vec();
    }
    picked.sort_by(|a, b| ((hsv(*a).0 - 45.0).rem_euclid(360.0)).total_cmp(&(hsv(*b).0 - 45.0).rem_euclid(360.0)));
    picked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trailing_whitespace() {
        assert_eq!(trailing_ws("let x = 1;"), None);
        assert_eq!(trailing_ws("let x = 1; \t "), Some(10));
        assert_eq!(trailing_ws("    "), Some(0));
        assert_eq!(trailing_ws("中文  "), Some(2), "counted in chars");
        assert_eq!(trailing_ws(""), None);
    }

    fn colors(text: &str) -> Vec<(Range<usize>, [u8; 4])> {
        find_colors(text).into_iter().map(|(r, c)| (r, c.to_srgba_unmultiplied())).collect()
    }

    #[test]
    fn hex_colors() {
        assert_eq!(colors("color: #abc;"), [(7..11, [0xaa, 0xbb, 0xcc, 255])]);
        assert_eq!(colors("#a1b2c3"), [(0..7, [0xa1, 0xb2, 0xc3, 255])]);
        assert_eq!(colors("x=#ff000080"), [(2..11, [0xff, 0, 0, 0x80])]);
        assert_eq!(colors("#fff8 #12345"), [(0..5, [255, 255, 255, 0x88])], "five digits isn't a color");
        assert!(colors("#[derive(Debug)] &#123; a#bcd #abcg").is_empty());
    }

    #[test]
    fn functional_colors() {
        assert_eq!(colors("rgb(1,2,3)"), [(0..10, [1, 2, 3, 255])]);
        assert_eq!(colors("a: rgba(255, 0, 0, 0.5);"), [(3..23, [255, 0, 0, 128])]);
        assert_eq!(colors("rgb(100% 0% 0% / 50%)"), [(0..21, [255, 0, 0, 128])]);
        assert_eq!(colors("hsl(120, 100%, 50%)"), [(0..19, [0, 255, 0, 255])]);
        assert_eq!(colors("HSLA(240deg 100% 25% / 1)"), [(0..25, [0, 0, 128, 255])]);
        assert!(colors("rgb(1,2) hsl(a,b,c) crgb(1,2,3) rgb(1,2,3").is_empty());
    }

    #[test]
    fn palettes_are_distinct_and_readable() {
        for theme in crate::highlight::theme_names() {
            let p = bracket_palette(theme);
            assert!((4..=6).contains(&p.len()), "{theme}: {}", p.len());
            for (i, a) in p.iter().enumerate() {
                assert!(p[i + 1..].iter().all(|b| b != a), "{theme}");
            }
        }
    }
}
