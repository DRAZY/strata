//! CJK fallback fonts. egui's built-in fonts have no CJK glyphs, so the first
//! time CJK text appears we add system fonts as fallbacks (loaded lazily:
//! they're tens of MB and most files never need them).

use egui::epaint::text::{FontData, FontInsert, FontPriority, InsertFontFamily};
use egui::{Context, FontFamily, Id};

/// Per script, the first file that exists is used.
#[cfg(target_os = "macos")]
const CANDIDATES: &[&[&str]] = &[
    // Chinese + Japanese kana
    &["/System/Library/Fonts/Hiragino Sans GB.ttc", "/System/Library/Fonts/Supplemental/Songti.ttc"],
    // Korean
    &["/System/Library/Fonts/AppleSDGothicNeo.ttc", "/System/Library/Fonts/Supplemental/AppleGothic.ttf"],
];

#[cfg(windows)]
const CANDIDATES: &[&[&str]] = &[
    &["C:\\Windows\\Fonts\\msyh.ttc", "C:\\Windows\\Fonts\\simsun.ttc"],
    &["C:\\Windows\\Fonts\\YuGothM.ttc", "C:\\Windows\\Fonts\\meiryo.ttc", "C:\\Windows\\Fonts\\msgothic.ttc"],
    &["C:\\Windows\\Fonts\\malgun.ttf", "C:\\Windows\\Fonts\\gulim.ttc"],
];

#[cfg(not(any(target_os = "macos", windows)))]
const CANDIDATES: &[&[&str]] = &[&[
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
]];

pub fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x11FF       // Hangul Jamo
        | 0x2E80..=0x9FFF     // CJK radicals, kana, ideographs, punctuation
        | 0xAC00..=0xD7AF     // Hangul syllables
        | 0xF900..=0xFAFF     // CJK compatibility ideographs
        | 0xFF00..=0xFFEF     // full-width forms
        | 0x20000..=0x2FA1F)  // ideograph extensions
}

/// Load the fallbacks once, the first time `text` contains CJK.
pub fn ensure_cjk(ctx: &Context, text: &str) {
    let loaded = Id::new("strata-cjk-fonts");
    if ctx.data(|d| d.get_temp::<bool>(loaded).is_some()) || !text.chars().any(is_cjk) {
        return;
    }
    ctx.data_mut(|d| d.insert_temp(loaded, true));
    for (i, files) in CANDIDATES.iter().enumerate() {
        let Some(bytes) = files.iter().find_map(|f| std::fs::read(f).ok()) else { continue };
        let families = [FontFamily::Monospace, FontFamily::Proportional]
            .into_iter()
            .map(|family| InsertFontFamily { family, priority: FontPriority::Lowest })
            .collect();
        ctx.add_font(FontInsert::new(&format!("cjk-{i}"), FontData::from_owned(bytes), families));
    }
}

/// macOS's San Francisco font, which draws the shortcut glyphs (⇧ ⌥ ⌃ ⌘)
/// egui's own fonts lack, at matching size and baseline.
fn symbol_fallback() -> Option<Vec<u8>> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    ["/System/Library/Fonts/SFNS.ttf", "/System/Library/Fonts/Apple Symbols.ttf"].iter().find_map(|f| std::fs::read(f).ok())
}

/// A font file installed on this computer.
#[derive(Clone, Debug)]
pub struct InstalledFont {
    pub name: String,
    pub path: std::path::PathBuf,
}

fn font_dirs() -> Vec<std::path::PathBuf> {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let mut dirs: Vec<std::path::PathBuf> = if cfg!(target_os = "macos") {
        vec!["/System/Library/Fonts".into(), "/System/Library/Fonts/Supplemental".into(), "/Library/Fonts".into()]
    } else if cfg!(windows) {
        let mut v: Vec<std::path::PathBuf> = vec!["C:\\Windows\\Fonts".into()];
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            v.push(std::path::PathBuf::from(local).join("Microsoft\\Windows\\Fonts"));
        }
        v
    } else {
        vec!["/usr/share/fonts".into(), "/usr/local/share/fonts".into()]
    };
    if let Some(h) = home {
        dirs.push(if cfg!(target_os = "macos") { h.join("Library/Fonts") } else { h.join(".local/share/fonts") });
    }
    dirs
}

/// Fonts found in the system and user font folders, named after their files
/// ("SF-Mono-Regular.otf" → "SF Mono Regular"), sorted, coding fonts included.
pub fn installed_fonts() -> Vec<InstalledFont> {
    let mut out: Vec<InstalledFont> = Vec::new();
    let mut stack = font_dirs();
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let path = e.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let ext = path.extension().and_then(|x| x.to_str()).map(str::to_lowercase);
            if !matches!(ext.as_deref(), Some("ttf" | "otf" | "ttc")) {
                continue;
            }
            let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            // Skip obvious non-text fonts (emoji, symbols, last-resort).
            if stem.contains("Emoji") || stem.starts_with("LastResort") || stem.contains("Symbol") {
                continue;
            }
            let name = stem.replace(['-', '_'], " ");
            out.push(InstalledFont { name, path });
        }
    }
    out.sort_by_key(|f| f.name.to_lowercase());
    out.dedup_by(|a, b| a.name.eq_ignore_ascii_case(&b.name));
    out
}

/// Resolve a font setting (a name from `installed_fonts`, or a file path).
pub fn find_font(setting: &str) -> Option<std::path::PathBuf> {
    if setting.is_empty() {
        return None;
    }
    let p = std::path::Path::new(setting);
    if p.is_file() {
        return Some(p.to_owned());
    }
    installed_fonts().into_iter().find(|f| f.name.eq_ignore_ascii_case(setting)).map(|f| f.path)
}

/// Rebuild the font set: the chosen editor font leads the monospace family,
/// the chosen interface font leads the proportional one, egui's fonts stay
/// as fallbacks. Returns a message for each font that couldn't be loaded.
pub fn apply(ctx: &Context, editor: Option<&std::path::Path>, ui: Option<&std::path::Path>) -> Vec<String> {
    use egui::FontDefinitions;
    use std::sync::Arc;
    let mut defs = FontDefinitions::default();
    let mut errors = Vec::new();
    for (key, path, family) in [("strata-editor", editor, FontFamily::Monospace), ("strata-ui", ui, FontFamily::Proportional)] {
        let Some(path) = path else { continue };
        match std::fs::read(path) {
            Ok(bytes) => {
                defs.font_data.insert(key.to_owned(), Arc::new(FontData::from_owned(bytes)));
                defs.families.entry(family).or_default().insert(0, key.to_owned());
            }
            Err(e) => errors.push(format!("{}: {e}", path.display())),
        }
    }
    if let Some(bytes) = symbol_fallback() {
        defs.font_data.insert("macos-symbols".to_owned(), Arc::new(FontData::from_owned(bytes)));
        defs.families.entry(FontFamily::Proportional).or_default().push("macos-symbols".to_owned());
    }
    ctx.set_fonts(defs);
    // CJK fallbacks were part of the old set; load them again when needed.
    ctx.data_mut(|d| d.remove::<bool>(Id::new("strata-cjk-fonts")));
    errors
}

/// Interface text sizes, scaled from one base size.
pub fn apply_ui_size(ctx: &Context, size: f32) {
    use egui::TextStyle;
    ctx.all_styles_mut(|style| {
        for (ts, font) in style.text_styles.iter_mut() {
            font.size = match ts {
                TextStyle::Small => size * 0.78,
                TextStyle::Heading => size * 1.45,
                _ => size,
            };
        }
    });
}
