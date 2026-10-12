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

/// macOS shortcut glyphs (⇧ ⌥ ⌃) aren't in egui's fonts; Apple Symbols has them.
pub fn install_symbol_fallback(ctx: &Context) {
    if !cfg!(target_os = "macos") {
        return;
    }
    let Ok(bytes) = std::fs::read("/System/Library/Fonts/Apple Symbols.ttf") else { return };
    let families = vec![InsertFontFamily { family: FontFamily::Proportional, priority: FontPriority::Lowest }];
    ctx.add_font(FontInsert::new("apple-symbols", FontData::from_owned(bytes), families));
}
