//! Characters that hide or reorder text: bidirectional controls, which
//! can make code read in a different order from the one it runs in (the
//! "Trojan Source" attack, CVE-2021-42574), and invisible ones (zero-width,
//! soft hyphen, a byte order mark mid-text), which make two texts that
//! look the same differ.
//!
//! The editor and Compare draw each one as a small labeled box that takes
//! its own columns (like a tab), so it can't vanish or move the text around
//! it. The text itself never changes: the box is only how it's drawn.

use crate::buffer::Buffer;
use egui::{Align2, Color32, FontId, Rect, Stroke};

/// What a hidden character is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hidden {
    /// The label drawn in its box ("RLO").
    pub short: &'static str,
    /// Its Unicode name.
    pub name: &'static str,
    /// A bidirectional control (it can reorder text), not just an invisible one.
    pub reorders: bool,
}

const fn h(short: &'static str, name: &'static str, reorders: bool) -> Option<Hidden> {
    Some(Hidden { short, name, reorders })
}

/// `c`'s description when it's a hidden or reordering character.
pub fn info(c: char) -> Option<Hidden> {
    match c {
        '\u{202A}' => h("LRE", "LEFT-TO-RIGHT EMBEDDING", true),
        '\u{202B}' => h("RLE", "RIGHT-TO-LEFT EMBEDDING", true),
        '\u{202C}' => h("PDF", "POP DIRECTIONAL FORMATTING", true),
        '\u{202D}' => h("LRO", "LEFT-TO-RIGHT OVERRIDE", true),
        '\u{202E}' => h("RLO", "RIGHT-TO-LEFT OVERRIDE", true),
        '\u{2066}' => h("LRI", "LEFT-TO-RIGHT ISOLATE", true),
        '\u{2067}' => h("RLI", "RIGHT-TO-LEFT ISOLATE", true),
        '\u{2068}' => h("FSI", "FIRST STRONG ISOLATE", true),
        '\u{2069}' => h("PDI", "POP DIRECTIONAL ISOLATE", true),
        '\u{200E}' => h("LRM", "LEFT-TO-RIGHT MARK", true),
        '\u{200F}' => h("RLM", "RIGHT-TO-LEFT MARK", true),
        '\u{061C}' => h("ALM", "ARABIC LETTER MARK", true),
        '\u{200B}' => h("ZWSP", "ZERO WIDTH SPACE", false),
        '\u{200C}' => h("ZWNJ", "ZERO WIDTH NON-JOINER", false),
        '\u{200D}' => h("ZWJ", "ZERO WIDTH JOINER", false),
        '\u{2060}' => h("WJ", "WORD JOINER", false),
        // A file's own byte order mark is dropped when it's read (see
        // `Buffer::decode`), so one still in the text is never at the file's start.
        '\u{FEFF}' => h("BOM", "ZERO WIDTH NO-BREAK SPACE", false),
        '\u{00AD}' => h("SHY", "SOFT HYPHEN", false),
        _ => None,
    }
}

pub fn is_hidden(c: char) -> bool {
    info(c).is_some()
}

/// Display columns `c` takes when hidden characters are shown: one per
/// letter of its label, so the label fits in a smaller font with room around it.
pub fn cols(c: char) -> usize {
    info(c).map_or(1, |h| h.short.len().max(2))
}

/// The hover text for `c`, e.g. "U+202E RIGHT-TO-LEFT OVERRIDE: can make
/// text display in a different order".
pub fn describe(c: char) -> Option<String> {
    let h = info(c)?;
    let why = if h.reorders { "can make text display in a different order" } else { "invisible, so text that looks the same can differ" };
    Some(format!("U+{:04X} {}: {why}", c as u32, h.name))
}

/// Hidden characters in `chunks`. Mostly-ASCII text is skipped a chunk at a
/// time, so this runs at memory speed.
pub fn count<'a>(chunks: impl Iterator<Item = &'a str>) -> usize {
    chunks.filter(|c| !c.is_ascii()).map(|c| c.chars().filter(|&c| is_hidden(c)).count()).sum()
}

/// The hidden character nearest after char `from` (or before it), wrapping
/// around the end of the text. Scans the whole text at worst, so it's for a
/// command, not every frame.
pub fn step(buf: &Buffer, from: usize, forward: bool) -> Option<usize> {
    let len = buf.len_chars();
    let from = from.min(len);
    let first = |range: std::ops::Range<usize>| {
        let mut pos = range.start;
        for chunk in buf.chunks_in(range) {
            if chunk.is_ascii() {
                pos += chunk.len();
                continue;
            }
            for c in chunk.chars() {
                if is_hidden(c) {
                    return Some(pos);
                }
                pos += 1;
            }
        }
        None
    };
    let last = |range: std::ops::Range<usize>| {
        let (mut pos, mut found) = (range.start, None);
        for chunk in buf.chunks_in(range) {
            if chunk.is_ascii() {
                pos += chunk.len();
                continue;
            }
            for c in chunk.chars() {
                if is_hidden(c) {
                    found = Some(pos);
                }
                pos += 1;
            }
        }
        found
    };
    if forward { first(from..len).or_else(|| first(0..from)) } else { last(0..from).or_else(|| last(from..len)) }
}

/// Texts up to this many chars are counted on the spot (about a
/// millisecond); bigger ones on a thread, so typing in a huge file never waits.
pub const COUNT_NOW: usize = 1 << 20;

/// A text's hidden-character count for the status bar, kept for the
/// text version it was made for.
#[derive(Default)]
pub struct Counter {
    done: Option<(u64, usize)>,
    running: Option<(u64, std::sync::mpsc::Receiver<usize>)>,
}

impl Counter {
    /// The count for `version` of `buf`; while a big text is recounted, the
    /// last count known (None before the first). `ctx` repaints when it's ready.
    pub fn get(&mut self, buf: &Buffer, version: u64, ctx: &egui::Context) -> Option<usize> {
        if let Some((v, rx)) = &self.running {
            match rx.try_recv() {
                Ok(n) => {
                    self.done = Some((*v, n));
                    self.running = None;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.running = None,
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
        if let Some((v, n)) = self.done
            && v == version
        {
            return Some(n);
        }
        if buf.len_chars() <= COUNT_NOW {
            let n = count(buf.chunks_in(0..buf.len_chars()));
            self.done = Some((version, n));
            return Some(n);
        }
        // One count at a time: edits during it are caught by the next one.
        if self.running.is_none() {
            let (tx, rx) = std::sync::mpsc::channel();
            let (rope, ctx) = (buf.snapshot(), ctx.clone());
            let spawned = std::thread::Builder::new().name("hidden-count".into()).spawn(move || {
                let _ = tx.send(count(rope.chunks()));
                ctx.request_repaint();
            });
            if spawned.is_ok() {
                self.running = Some((version, rx));
            }
        }
        self.done.map(|(_, n)| n)
    }
}

// ---- interface text -------------------------------------------------------------
//
// Everything that isn't the editor or Compare (the Markdown preview, help,
// hover and completion popups, tab titles, the side bar, the palette, Find
// in Files, status messages) shows text through these. A hidden character
// becomes its label in plain letters on a warning tint: the letters exist
// in every interface font (symbol brackets don't), and the control itself
// never reaches the shaper, so it can't reorder what's around it. The text
// itself is never changed; only how it's drawn.

/// Whether `s` holds any hidden character (fast for ASCII).
pub fn any(s: &str) -> bool {
    !s.is_ascii() && s.chars().any(is_hidden)
}

/// What `c` shows as in interface text: its label between thin spaces.
fn marker(h: Hidden) -> String {
    format!("\u{2009}{}\u{2009}", h.short)
}

/// Chars `c` takes in interface text (1 unless it's hidden).
fn shown_len(c: char) -> usize {
    info(c).map_or(1, |h| h.short.len() + 2)
}

/// The warning color for text drawn in `color`: darker on light backgrounds.
fn warn_for(color: Color32) -> Color32 {
    let light = color.r() as u32 + color.g() as u32 + color.b() as u32 <= 3 * 128;
    crate::view::marker_color(2, light).unwrap_or(color)
}

/// Append `text` to `job` in `fmt`, each hidden character as its marker.
pub fn append(job: &mut egui::text::LayoutJob, text: &str, leading_space: f32, fmt: egui::text::TextFormat) {
    if !any(text) {
        job.append(text, leading_space, fmt);
        return;
    }
    let warn = warn_for(fmt.color);
    let mut mark = fmt.clone();
    mark.color = warn;
    mark.background = warn.gamma_multiply(0.18);
    // Same size as the text: a smaller font would sit off its baseline in some layouts.
    mark.expand_bg = 0.0;
    mark.italics = false;
    let (mut start, mut lead) = (0, leading_space);
    for (b, c) in text.char_indices() {
        let Some(h) = info(c) else { continue };
        if b > start {
            job.append(&text[start..b], lead, fmt.clone());
            lead = 0.0;
        }
        job.append(&marker(h), lead, mark.clone());
        lead = 0.0;
        start = b + c.len_utf8();
    }
    if start < text.len() {
        job.append(&text[start..], lead, fmt);
    }
}

/// `text` laid out in one format, hidden characters as markers.
pub fn job(text: &str, fmt: egui::text::TextFormat) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    append(&mut job, text, 0.0, fmt);
    job
}

/// For a label or button: `text` as is when it has no hidden characters,
/// otherwise laid out in `font` and `color` with markers. Labels keep their
/// usual look; only text that needs markers takes the longer path.
pub fn widget(text: &str, font: FontId, color: Color32) -> egui::WidgetText {
    if !any(text) {
        return egui::RichText::new(text).font(font).color(color).into();
    }
    let mut j = job(text, egui::text::TextFormat::simple(font, color));
    j.wrap.max_width = f32::INFINITY;
    j.into()
}

/// [`widget`] in the interface's body font and text color.
pub fn label(ui: &egui::Ui, text: &str) -> egui::WidgetText {
    if !any(text) {
        return text.into();
    }
    widget(text, egui::TextStyle::Body.resolve(ui.style()), ui.visuals().text_color())
}

/// `s` with each hidden character spelled `[RLO]`, for text that can only be
/// a plain string (window titles, tooltips, system notifications).
pub fn plain(s: &str) -> std::borrow::Cow<'_, str> {
    if !any(s) {
        return s.into();
    }
    s.chars().map(|c| info(c).map_or_else(|| c.to_string(), |h| format!("[{}]", h.short))).collect::<String>().into()
}

/// The char of `text` at char `shown` of its laid-out form (see `append`),
/// for hit-testing links in a galley.
pub fn source_index(text: &str, shown: usize) -> usize {
    let (mut at, mut k) = (0, 0);
    for c in text.chars() {
        at += shown_len(c);
        if at > shown {
            return k;
        }
        k += 1;
    }
    k
}

/// Draw `c`'s box over `cell` (the columns it takes): a rounded outline in
/// `color` with the label inside, in a font sized to fit.
pub fn paint(painter: &egui::Painter, cell: Rect, c: char, color: Color32, font_size: f32) {
    let Some(h) = info(c) else { return };
    let r = cell.shrink2(egui::vec2(1.0, 0.0));
    painter.rect_filled(r, 3.0, color.gamma_multiply(0.14));
    painter.rect_stroke(r, 3.0, Stroke::new(1.0, color), egui::StrokeKind::Inside);
    painter.text(r.center(), Align2::CENTER_CENTER, h.short, FontId::proportional((font_size * 0.62).max(7.0)), color);
}

/// A job's laid-out text, after checking no raw hidden character is left
/// in it (for tests of each surface).
#[cfg(test)]
pub fn shown(job: &egui::text::LayoutJob) -> String {
    assert!(!any(&job.text), "a raw hidden character reached the layout: {:?}", job.text);
    job.text.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BIDI: [char; 12] = ['\u{202A}', '\u{202B}', '\u{202C}', '\u{202D}', '\u{202E}', '\u{2066}', '\u{2067}', '\u{2068}', '\u{2069}', '\u{200E}', '\u{200F}', '\u{061C}'];
    const INVISIBLE: [char; 6] = ['\u{200B}', '\u{200C}', '\u{200D}', '\u{2060}', '\u{FEFF}', '\u{00AD}'];

    #[test]
    fn every_class_is_found_and_named() {
        for c in BIDI {
            let h = info(c).unwrap_or_else(|| panic!("{:04X}", c as u32));
            assert!(h.reorders, "{:04X}", c as u32);
            assert!(describe(c).unwrap().ends_with("can make text display in a different order"));
        }
        for c in INVISIBLE {
            assert!(!info(c).unwrap_or_else(|| panic!("{:04X}", c as u32)).reorders);
        }
        assert_eq!(describe('\u{202E}').unwrap(), "U+202E RIGHT-TO-LEFT OVERRIDE: can make text display in a different order");
        assert_eq!(info('\u{200B}').unwrap().short, "ZWSP");
        for c in ['a', ' ', '\t', '\u{00A0}', '\u{2009}', '\u{202F}', 'é', '中', '\u{1F600}', '\u{05D0}'] {
            assert!(!is_hidden(c), "{:04X}", c as u32);
        }
        // Every label fits its columns.
        for c in BIDI.into_iter().chain(INVISIBLE) {
            assert!(cols(c) >= info(c).unwrap().short.len() && cols(c) >= 2);
        }
        assert_eq!(cols('a'), 1);
    }

    #[test]
    fn counting_and_stepping() {
        let text = "a\u{202E}b \u{2066}c\u{2069}\nplain ascii line\nd\u{200B}";
        assert_eq!(count([text].into_iter()), 4);
        assert_eq!(count(["only ascii", "and é ü"].into_iter()), 0);
        let buf = Buffer::from_str(text);
        let at: Vec<usize> = text.chars().enumerate().filter(|(_, c)| is_hidden(*c)).map(|(i, _)| i).collect();
        assert_eq!(step(&buf, 0, true), Some(at[0]));
        assert_eq!(step(&buf, at[0] + 1, true), Some(at[1]));
        assert_eq!(step(&buf, at[3] + 1, true), Some(at[0]), "wraps to the top");
        assert_eq!(step(&buf, at[1], false), Some(at[0]));
        assert_eq!(step(&buf, 0, false), Some(at[3]), "wraps to the bottom");
        assert_eq!(step(&Buffer::from_str("nothing here\n"), 3, true), None);
    }

    #[test]
    fn big_texts_are_counted_off_the_ui_thread() {
        let ctx = egui::Context::default();
        let line = "let x = 1; // ascii filler for a big file\n";
        let mut text = line.repeat(COUNT_NOW / line.len() + 10);
        text.push_str("a\u{202E}b\u{200B}\n");
        let buf = Buffer::from_str(&text);
        let mut counter = Counter::default();
        let start = std::time::Instant::now();
        let first = counter.get(&buf, 1, &ctx);
        assert!(start.elapsed() < std::time::Duration::from_millis(50), "the first call doesn't wait for the count");
        assert_eq!(first, None);
        let mut n = None;
        while n.is_none() && start.elapsed() < std::time::Duration::from_secs(10) {
            std::thread::sleep(std::time::Duration::from_millis(5));
            n = counter.get(&buf, 1, &ctx);
        }
        assert_eq!(n, Some(2));
        // A new version shows the old count until the recount arrives.
        assert_eq!(counter.get(&buf, 2, &ctx), Some(2));
        // Small texts are counted at once.
        let mut small = Counter::default();
        assert_eq!(small.get(&Buffer::from_str("x\u{2066}y"), 7, &ctx), Some(1));
    }

    #[test]
    fn interface_text_gets_markers_and_keeps_its_order() {
        let fmt = egui::text::TextFormat::simple(FontId::proportional(14.0), Color32::WHITE);
        let j = job("a\u{202E}bc\u{200B}", fmt.clone());
        assert_eq!(shown(&j), "a\u{2009}RLO\u{2009}bc\u{2009}ZWSP\u{2009}");
        assert_eq!(j.sections.len(), 4);
        assert_ne!(j.sections[1].format.background, Color32::TRANSPARENT, "markers sit on a tint");
        assert_eq!(j.sections[1].format.color, crate::view::marker_color(2, false).unwrap());
        // Dark text (a light theme) gets the darker warning color.
        let dark = job("\u{2066}", egui::text::TextFormat::simple(FontId::proportional(14.0), Color32::BLACK));
        assert_eq!(dark.sections[0].format.color, crate::view::marker_color(2, true).unwrap());
        // Clean text is one section, untouched.
        assert_eq!(job("plain é", fmt).sections.len(), 1);
        assert_eq!(plain("x\u{202E}y"), "x[RLO]y");
        assert!(matches!(plain("clean"), std::borrow::Cow::Borrowed(_)));
        // Galley chars map back to the source: "a", then RLO's 5, then "b".
        let t = "a\u{202E}b";
        assert_eq!((0..8).map(|i| source_index(t, i)).collect::<Vec<_>>(), [0, 1, 1, 1, 1, 1, 2, 3]);
    }

    #[test]
    fn a_files_own_byte_order_mark_isnt_counted() {
        let (buf, _) = Buffer::decode(b"\xEF\xBB\xBFfn main() {}\n");
        assert_eq!(count(buf.chunks_in(0..buf.len_chars())), 0);
        // A second one, mid-text, is.
        let (buf, _) = Buffer::decode("\u{FEFF}a\u{FEFF}b".as_bytes());
        assert_eq!(count(buf.chunks_in(0..buf.len_chars())), 1);
    }
}
