//! Fonts: the coding fonts bundled with Strata, installed system fonts, the
//! editor and interface font settings, and CJK fallbacks. egui's built-in
//! fonts have no CJK glyphs, so the first time CJK text appears we add system
//! fonts as fallbacks (loaded lazily: they're tens of MB and most files never
//! need them).

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

/// Load the fallbacks once, the first time `text` contains CJK. Cheap
/// enough to call on every label every frame: ASCII returns at once.
pub fn ensure_cjk(ctx: &Context, text: &str) {
    if text.is_ascii() || !text.chars().any(is_cjk) {
        return;
    }
    let loaded = Id::new("strata-cjk-fonts");
    if ctx.data(|d| d.get_temp::<bool>(loaded).is_some()) {
        return;
    }
    ctx.data_mut(|d| d.insert_temp(loaded, true));
    // The new fonts take effect next frame.
    ctx.request_repaint();
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

/// A font that ships inside Strata. All are under the SIL Open Font License
/// 1.1; the texts are in `assets/fonts` and Help ▸ Licenses. Only the regular
/// weight ships: egui draws one weight (italics are slanted by the renderer).
pub struct BundledFont {
    pub name: &'static str,
    pub data: &'static [u8],
    /// Monospaced (offered first for the editor).
    pub mono: bool,
    pub license: &'static str,
    /// Shown beside the name in the font picker.
    pub note: &'static str,
}

pub static BUNDLED: &[BundledFont] = &[
    BundledFont {
        name: "JetBrains Mono",
        data: include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf"),
        mono: true,
        license: include_str!("../assets/fonts/JetBrainsMono-OFL.txt"),
        // egui shapes text, so the coding ligatures (=> != ===) draw, one cell per character.
        note: "bundled · ligatures",
    },
    BundledFont {
        name: "Fira Code",
        data: include_bytes!("../assets/fonts/FiraCode-Regular.ttf"),
        mono: true,
        license: include_str!("../assets/fonts/FiraCode-OFL.txt"),
        note: "bundled · ligatures",
    },
    BundledFont {
        name: "IBM Plex Mono",
        data: include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf"),
        mono: true,
        license: include_str!("../assets/fonts/IBMPlexMono-OFL.txt"),
        note: "bundled",
    },
    BundledFont {
        name: "Source Code Pro",
        data: include_bytes!("../assets/fonts/SourceCodePro-Regular.ttf"),
        mono: true,
        license: include_str!("../assets/fonts/SourceCodePro-OFL.txt"),
        note: "bundled",
    },
    BundledFont {
        name: "Inter",
        data: include_bytes!("../assets/fonts/Inter-Regular.ttf"),
        mono: false,
        license: include_str!("../assets/fonts/Inter-OFL.txt"),
        note: "bundled",
    },
];

/// Where a font comes from.
#[derive(Clone, Debug)]
pub enum FontSource {
    Bundled(&'static BundledFont),
    File(std::path::PathBuf),
}

impl std::fmt::Debug for BundledFont {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name)
    }
}

/// A row in the font pickers.
#[derive(Clone, Debug)]
pub struct FontChoice {
    /// What the setting stores.
    pub name: String,
    pub note: &'static str,
}

/// What the font pickers offer after "Default": the bundled fonts first
/// (monospaced ones first for the editor, Inter first for the interface),
/// then every installed font.
pub fn font_choices(editor: bool) -> Vec<FontChoice> {
    let mut bundled: Vec<&BundledFont> = BUNDLED.iter().collect();
    bundled.sort_by_key(|f| f.mono != editor);
    let mut out: Vec<FontChoice> = bundled.into_iter().map(|f| FontChoice { name: f.name.to_owned(), note: f.note }).collect();
    out.extend(installed_fonts().into_iter().map(|f| FontChoice { name: f.name, note: "" }));
    out
}

/// Resolve a font setting: a bundled font's name, a font file path, or a
/// name from `installed_fonts`.
pub fn find_font(setting: &str) -> Option<FontSource> {
    let setting = setting.trim();
    if setting.is_empty() {
        return None;
    }
    let squash = |s: &str| s.chars().filter(|c| !c.is_whitespace() && *c != '-' && *c != '_').collect::<String>().to_lowercase();
    if let Some(f) = BUNDLED.iter().find(|f| squash(f.name) == squash(setting)) {
        return Some(FontSource::Bundled(f));
    }
    let p = std::path::Path::new(setting);
    if p.is_file() {
        return Some(FontSource::File(p.to_owned()));
    }
    installed_fonts().into_iter().find(|f| f.name.eq_ignore_ascii_case(setting)).map(|f| FontSource::File(f.path))
}

/// Rebuild the font set: the chosen editor font leads the monospace family,
/// the chosen interface font leads the proportional one, egui's fonts stay
/// as fallbacks (for glyphs the chosen font lacks). Returns a message for
/// each font that couldn't be loaded.
pub fn apply(ctx: &Context, editor: Option<&FontSource>, ui: Option<&FontSource>) -> Vec<String> {
    let (defs, errors) = definitions(editor, ui);
    ctx.set_fonts(defs);
    // CJK fallbacks were part of the old set; load them again when needed.
    ctx.data_mut(|d| d.remove::<bool>(Id::new("strata-cjk-fonts")));
    errors
}

fn definitions(editor: Option<&FontSource>, ui: Option<&FontSource>) -> (egui::FontDefinitions, Vec<String>) {
    use std::sync::Arc;
    let mut defs = egui::FontDefinitions::default();
    let mut errors = Vec::new();
    for (key, source, family) in [("strata-editor", editor, FontFamily::Monospace), ("strata-ui", ui, FontFamily::Proportional)] {
        let data = match source {
            None => continue,
            Some(FontSource::Bundled(f)) => FontData::from_static(f.data),
            Some(FontSource::File(path)) => match std::fs::read(path) {
                Ok(bytes) => FontData::from_owned(bytes),
                Err(e) => {
                    errors.push(format!("{}: {e}", path.display()));
                    continue;
                }
            },
        };
        defs.font_data.insert(key.to_owned(), Arc::new(data));
        defs.families.entry(family).or_default().insert(0, key.to_owned());
    }
    if let Some(bytes) = symbol_fallback() {
        defs.font_data.insert("macos-symbols".to_owned(), Arc::new(FontData::from_owned(bytes)));
        defs.families.entry(FontFamily::Proportional).or_default().push("macos-symbols".to_owned());
    }
    (defs, errors)
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

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Color32, FontId, RawInput};

    fn context_with(editor: Option<&FontSource>) -> Context {
        let ctx = Context::default();
        assert!(apply(&ctx, editor, None).is_empty());
        // No GPU here: discard font-atlas uploads instead of applying them.
        ctx.run_ui(RawInput::default(), |_| {}).textures_delta.clear();
        ctx
    }

    fn metrics(ctx: &Context) -> (f32, f32) {
        let font = FontId::monospace(14.0);
        ctx.fonts_mut(|f| (f.glyph_width(&font, 'M'), f.row_height(&font)))
    }

    #[test]
    fn bundled_fonts_are_offered_first_and_found_by_name() {
        let editor = font_choices(true);
        let mono = BUNDLED.iter().filter(|f| f.mono).count();
        assert!(mono >= 4);
        assert!(editor[..mono].iter().all(|c| BUNDLED.iter().any(|b| b.mono && b.name == c.name)), "monospaced bundled fonts lead the editor list");
        assert_eq!(font_choices(false)[0].name, "Inter", "Inter leads the interface list");
        assert!(matches!(find_font("JetBrains Mono"), Some(FontSource::Bundled(f)) if f.name == "JetBrains Mono"));
        assert!(matches!(find_font("jetbrainsmono"), Some(FontSource::Bundled(_))), "spacing and case are forgiven");
        assert!(matches!(find_font(" fira-code "), Some(FontSource::Bundled(f)) if f.name == "Fira Code"));
        assert!(find_font("").is_none());
        assert!(find_font("No Such Font 123").is_none());
        for f in BUNDLED {
            assert!(f.license.contains("SIL Open Font License"), "{}", f.name);
        }
    }

    /// Every bundled font loads and actually changes the editor's metrics.
    #[test]
    fn bundled_fonts_change_rendering() {
        let default = metrics(&context_with(None));
        for f in BUNDLED.iter().filter(|f| f.mono) {
            let m = metrics(&context_with(Some(&FontSource::Bundled(f))));
            assert!(m.0 > 4.0 && m.1 > 10.0, "{}: {m:?}", f.name);
            assert_ne!(m, default, "{} renders differently from Hack", f.name);
        }
    }

    /// Coding ligatures (=> != ===) are drawn by egui's shaper, but every
    /// character keeps its own cell, so carets and selections stay on the grid.
    #[test]
    fn ligature_fonts_keep_one_cell_per_character() {
        for name in ["Fira Code", "JetBrains Mono"] {
            let ctx = context_with(find_font(name).as_ref());
            // The same length without anything that could form a ligature.
            let (text, plain) = ("a => b != c === d <= e", "a xx b xx c xxx d xx e");
            let font = FontId::monospace(14.0);
            let layout = |t: &str| ctx.fonts_mut(|f| f.layout_no_wrap(t.into(), font.clone(), Color32::WHITE));
            let (lig, reference) = (layout(text), layout(plain));
            for i in 0..=text.len() {
                let x = |g: &egui::Galley| g.pos_from_cursor(egui::text::CCursor::new(i)).min.x;
                assert!((x(&lig) - x(&reference)).abs() < 0.5, "{name}: char {i} at {} instead of {}", x(&lig), x(&reference));
            }
        }
    }

    #[test]
    fn a_missing_font_file_is_reported_and_skipped() {
        let missing = FontSource::File("/no/such/font.ttf".into());
        let (defs, errors) = definitions(Some(&missing), None);
        assert_eq!(errors.len(), 1);
        assert!(!defs.font_data.contains_key("strata-editor"), "egui's own font stays in charge");
    }
}
