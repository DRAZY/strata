//! A small PDF writer for printing: pages of positioned text runs, filled
//! rectangles and lines, with TrueType fonts embedded whole (CID-keyed,
//! Identity-H, with a ToUnicode map so text can be searched and copied).
//! Streams are deflated. Bold is drawn as filled-and-stroked glyphs and
//! italic as slanted ones, since only regular faces are bundled.
//!
//! Coordinates are PDF points with the origin at the top-left (y grows
//! down); they are flipped when written.

use flate2::Compression;
use flate2::write::ZlibEncoder;
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::io::Write as _;
use std::sync::Arc;

/// The parts of a TrueType font a PDF needs.
pub struct Font {
    data: Vec<u8>,
    units: f32,
    ascent: i16,
    descent: i16,
    cap_height: i16,
    bbox: [i16; 4],
    fixed: bool,
    /// Advance width of each glyph, in font units.
    advances: Vec<u16>,
    cmap: Cmap,
    pub name: String,
    /// Outline stroke added to every glyph, in ems: thickens a light face
    /// to read as regular on paper.
    pub weight: f32,
}

enum Cmap {
    /// Format 4: (end, start, delta, range offset, offset of that field).
    Segments(Vec<(u16, u16, i16, u16, usize)>),
    /// Format 12: (start, end, first glyph).
    Groups(Vec<(u32, u32, u32)>),
}

fn u16_at(d: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*d.get(at)?, *d.get(at + 1)?]))
}

fn i16_at(d: &[u8], at: usize) -> Option<i16> {
    u16_at(d, at).map(|v| v as i16)
}

fn u32_at(d: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes([*d.get(at)?, *d.get(at + 1)?, *d.get(at + 2)?, *d.get(at + 3)?]))
}

impl Font {
    /// Parse a TrueType (glyf-based) font. None if a needed table is missing.
    pub fn parse(data: Vec<u8>, name: &str) -> Option<Font> {
        let d = &data;
        let tables = u16_at(d, 4)? as usize;
        let mut dir: HashMap<[u8; 4], usize> = HashMap::new();
        for i in 0..tables {
            let rec = 12 + 16 * i;
            let tag: [u8; 4] = d.get(rec..rec + 4)?.try_into().ok()?;
            dir.insert(tag, u32_at(d, rec + 8)? as usize);
        }
        let head = *dir.get(b"head")?;
        let hhea = *dir.get(b"hhea")?;
        let hmtx = *dir.get(b"hmtx")?;
        let maxp = *dir.get(b"maxp")?;
        let cmap = *dir.get(b"cmap")?;
        dir.get(b"glyf")?;
        let units = u16_at(d, head + 18)? as f32;
        let bbox = [i16_at(d, head + 36)?, i16_at(d, head + 38)?, i16_at(d, head + 40)?, i16_at(d, head + 42)?];
        let (ascent, descent) = (i16_at(d, hhea + 4)?, i16_at(d, hhea + 6)?);
        let metrics = u16_at(d, hhea + 34)? as usize;
        let glyphs = u16_at(d, maxp + 4)? as usize;
        let mut advances = Vec::with_capacity(glyphs);
        let mut last = 0;
        for g in 0..glyphs {
            if g < metrics {
                last = u16_at(d, hmtx + 4 * g)?;
            }
            advances.push(last);
        }
        let cap_height = dir.get(b"OS/2").and_then(|&os2| (u16_at(d, os2)? >= 2).then(|| i16_at(d, os2 + 88)).flatten()).unwrap_or((ascent as f32 * 0.7) as i16);
        let fixed = dir.get(b"post").and_then(|&post| u32_at(d, post + 12)).is_some_and(|f| f != 0);
        let cmap = Self::parse_cmap(d, cmap)?;
        Some(Font { data, units, ascent, descent, cap_height, bbox, fixed, advances, cmap, name: name.to_owned(), weight: 0.0 })
    }

    fn parse_cmap(d: &[u8], at: usize) -> Option<Cmap> {
        let n = u16_at(d, at + 2)? as usize;
        let mut best: Option<(u8, usize)> = None;
        for i in 0..n {
            let rec = at + 4 + 8 * i;
            let (platform, encoding) = (u16_at(d, rec)?, u16_at(d, rec + 2)?);
            let sub = at + u32_at(d, rec + 4)? as usize;
            let format = u16_at(d, sub)?;
            let rank = match (platform, encoding, format) {
                (3, 10, 12) | (0, 4, 12) | (0, 6, 12) => 3,
                (3, 1, 4) | (0, 3, 4) => 2,
                (0, _, 4) => 1,
                _ => continue,
            };
            if best.is_none_or(|(r, _)| rank > r) {
                best = Some((rank, sub));
            }
        }
        let (_, sub) = best?;
        match u16_at(d, sub)? {
            4 => {
                let segs = u16_at(d, sub + 6)? as usize / 2;
                let ends = sub + 14;
                let starts = ends + 2 * segs + 2;
                let deltas = starts + 2 * segs;
                let offsets = deltas + 2 * segs;
                let mut v = Vec::with_capacity(segs);
                for s in 0..segs {
                    v.push((u16_at(d, ends + 2 * s)?, u16_at(d, starts + 2 * s)?, i16_at(d, deltas + 2 * s)?, u16_at(d, offsets + 2 * s)?, offsets + 2 * s));
                }
                Some(Cmap::Segments(v))
            }
            12 => {
                let n = u32_at(d, sub + 12)? as usize;
                let mut v = Vec::with_capacity(n);
                for g in 0..n {
                    let rec = sub + 16 + 12 * g;
                    v.push((u32_at(d, rec)?, u32_at(d, rec + 4)?, u32_at(d, rec + 8)?));
                }
                Some(Cmap::Groups(v))
            }
            _ => None,
        }
    }

    /// The glyph for `c`, if the font has one.
    pub fn glyph(&self, c: char) -> Option<u16> {
        let c = c as u32;
        let g = match &self.cmap {
            Cmap::Groups(groups) => {
                let i = groups.partition_point(|g| g.1 < c);
                let &(start, end, first) = groups.get(i)?;
                (start <= c && c <= end).then(|| first + (c - start))? as u16
            }
            Cmap::Segments(segs) => {
                if c > 0xFFFF {
                    return None;
                }
                let c = c as u16;
                let i = segs.partition_point(|s| s.0 < c);
                let &(end, start, delta, offset, at) = segs.get(i)?;
                if c < start || c > end {
                    return None;
                }
                if offset == 0 {
                    (c as i32 + delta as i32) as u16
                } else {
                    let g = u16_at(&self.data, at + offset as usize + 2 * (c - start) as usize)?;
                    if g == 0 {
                        return None;
                    }
                    (g as i32 + delta as i32) as u16
                }
            }
        };
        (g != 0 && (g as usize) < self.advances.len()).then_some(g)
    }

    /// Advance of glyph `g` in thousandths of the font size.
    pub fn advance(&self, g: u16) -> f32 {
        self.advances.get(g as usize).copied().unwrap_or(0) as f32 * 1000.0 / self.units
    }

    fn scaled(&self, v: i16) -> i32 {
        (v as f32 * 1000.0 / self.units).round() as i32
    }
}

/// The two faces egui bundles: Hack (monospace) and Ubuntu Light (proportional).
pub fn bundled_fonts() -> (Arc<Font>, Arc<Font>) {
    static FONTS: std::sync::LazyLock<(Arc<Font>, Arc<Font>)> = std::sync::LazyLock::new(|| {
        let defs = egui::FontDefinitions::default();
        let get = |key: &str, name: &str| {
            let data = defs.font_data.get(key).map(|f| f.font.to_vec()).unwrap_or_default();
            Arc::new(Font::parse(data, name).unwrap_or_else(|| panic!("bundled font {key} doesn't parse")))
        };
        let mut sans = Arc::try_unwrap(get("Ubuntu-Light", "Ubuntu-Light")).ok().expect("not shared yet");
        sans.weight = 0.018;
        (get("Hack", "Hack-Regular"), Arc::new(sans))
    });
    FONTS.clone()
}

#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
}

/// RGB, 0..1.
pub type Rgb = [f32; 3];

pub fn rgb(c: egui::Color32) -> Rgb {
    [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0]
}

struct FontUse {
    font: Arc<Font>,
    /// Glyphs used, with a char each maps back to (ToUnicode).
    used: BTreeMap<u16, char>,
}

/// A document being built.
pub struct Pdf {
    pub width: f32,
    pub height: f32,
    fonts: Vec<FontUse>,
    pages: Vec<String>,
}

fn num(v: f32) -> String {
    let s = format!("{:.2}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.to_owned() }
}

impl Pdf {
    /// `fonts`: the first is the default; the others stand in for glyphs it lacks.
    pub fn new(width: f32, height: f32, fonts: Vec<Arc<Font>>) -> Self {
        Self { width, height, fonts: fonts.into_iter().map(|font| FontUse { font, used: BTreeMap::new() }).collect(), pages: Vec::new() }
    }

    pub fn add_page(&mut self) -> usize {
        self.pages.push(String::new());
        self.pages.len() - 1
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// The font index and glyph that draw `c`, preferring `font`; '?' when none has it.
    fn resolve(&self, font: usize, c: char) -> (usize, u16) {
        if let Some(g) = self.fonts[font].font.glyph(c) {
            return (font, g);
        }
        for (i, f) in self.fonts.iter().enumerate() {
            if let Some(g) = f.font.glyph(c) {
                return (i, g);
            }
        }
        (font, self.fonts[font].font.glyph('?').unwrap_or(0))
    }

    /// Width of `s` in `font` at `size` points.
    pub fn width(&self, font: usize, size: f32, s: &str) -> f32 {
        s.chars()
            .filter(|c| !c.is_control())
            .map(|c| {
                let (f, g) = self.resolve(font, c);
                self.fonts[f].font.advance(g)
            })
            .sum::<f32>()
            * size
            / 1000.0
    }

    /// Draw `s` with its baseline at (x, y). With `cell`, each char takes
    /// that width (monospace grids keep their columns even where a fallback
    /// font draws a char). Returns the x after the text.
    #[allow(clippy::too_many_arguments)]
    pub fn text(&mut self, page: usize, x: f32, y: f32, font: usize, size: f32, color: Rgb, style: Style, s: &str, cell: Option<f32>) -> f32 {
        let mut runs: Vec<(usize, f32, Vec<u16>)> = Vec::new();
        let mut cx = x;
        for c in s.chars().filter(|c| !c.is_control()) {
            let (f, g) = self.resolve(font, c);
            self.fonts[f].used.entry(g).or_insert(c);
            let adv = cell.unwrap_or(self.fonts[f].font.advance(g) * size / 1000.0);
            match runs.last_mut() {
                Some((rf, _, glyphs)) if *rf == f && cell.is_none_or(|_| f == font) => glyphs.push(g),
                _ => runs.push((f, cx, vec![g])),
            }
            cx += adv;
        }
        let out = &mut self.pages[page];
        let [r, g, b] = color;
        let fy = self.height - y;
        for (f, rx, glyphs) in &runs {
            let hex: String = glyphs.iter().map(|g| format!("{g:04X}")).collect();
            let skew = if style.italic { 0.21 } else { 0.0 };
            let _ = write!(out, "BT /F{f} {} Tf {} {} {} rg ", num(size), num(r), num(g), num(b));
            // Text render mode stays set across BT/ET, so every run sets it.
            let weight = self.fonts[*f].font.weight + if style.bold { 0.035 } else { 0.0 };
            if weight > 0.0 {
                let _ = write!(out, "2 Tr {} w {} {} {} RG ", num(size * weight), num(r), num(g), num(b));
            } else {
                out.push_str("0 Tr ");
            }
            let _ = writeln!(out, "1 0 {} 1 {} {} Tm <{hex}> Tj ET", num(skew), num(*rx), num(fy));
        }
        if style.underline || style.strike {
            let dy = if style.underline { size * 0.12 } else { -size * 0.3 };
            self.hline(page, x, cx, y + dy, (size * 0.06).max(0.4), color);
        }
        cx
    }

    pub fn fill_rect(&mut self, page: usize, x: f32, y: f32, w: f32, h: f32, color: Rgb) {
        let [r, g, b] = color;
        let _ = writeln!(self.pages[page], "{} {} {} rg {} {} {} {} re f", num(r), num(g), num(b), num(x), num(self.height - y - h), num(w), num(h));
    }

    /// A horizontal rule from `x1` to `x2` at `y`.
    pub fn hline(&mut self, page: usize, x1: f32, x2: f32, y: f32, width: f32, color: Rgb) {
        let [r, g, b] = color;
        let y = num(self.height - y);
        let _ = writeln!(self.pages[page], "{} {} {} RG {} w {} {y} m {} {y} l S", num(r), num(g), num(b), num(width), num(x1), num(x2));
    }

    /// The finished file.
    pub fn finish(mut self, title: &str) -> Vec<u8> {
        if self.pages.is_empty() {
            self.add_page();
        }
        let mut w = Writer::default();
        w.out.extend_from_slice(b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n");
        // 1: catalog, 2: page tree, 3: info; the rest are allocated in order.
        let mut next = 4;
        let mut alloc = || {
            next += 1;
            next - 1
        };
        let font_ids: Vec<Option<usize>> = self.fonts.iter().map(|f| (!f.used.is_empty()).then(&mut alloc)).collect();
        let mut font_objs = Vec::new();
        for (i, f) in self.fonts.iter().enumerate() {
            if let Some(id) = font_ids[i] {
                font_objs.push((i, id, alloc(), alloc(), alloc(), alloc()));
                let _ = f;
            }
        }
        let page_ids: Vec<(usize, usize)> = self.pages.iter().map(|_| (alloc(), alloc())).collect();
        let resources = {
            let mut s = String::from("<< /Font <<");
            for (i, id) in font_ids.iter().enumerate() {
                if let Some(id) = id {
                    let _ = write!(s, " /F{i} {id} 0 R");
                }
            }
            s.push_str(" >> >>");
            s
        };
        w.obj(1, "<< /Type /Catalog /Pages 2 0 R >>");
        let kids: String = page_ids.iter().map(|(p, _)| format!("{p} 0 R ")).collect();
        w.obj(2, &format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids.trim_end(), page_ids.len()));
        let title_hex: String = std::iter::once(0xFEFFu16).chain(title.encode_utf16()).map(|u| format!("{u:04X}")).collect();
        w.obj(3, &format!("<< /Title <{title_hex}> /Producer (Strata) /Creator (Strata) >>"));
        for &(i, type0, cid, desc, file, tounicode) in &font_objs {
            let f = &self.fonts[i];
            let font = &f.font;
            let base = format!("/{}", font.name.replace(|c: char| !c.is_ascii_alphanumeric() && c != '-', ""));
            w.obj(type0, &format!("<< /Type /Font /Subtype /Type0 /BaseFont {base} /Encoding /Identity-H /DescendantFonts [{cid} 0 R] /ToUnicode {tounicode} 0 R >>"));
            let mut widths = String::new();
            for &g in f.used.keys() {
                let _ = write!(widths, "{g} [{}] ", font.advance(g).round() as i32);
            }
            let dw = font.glyph('M').map_or(500, |g| font.advance(g).round() as i32);
            w.obj(
                cid,
                &format!(
                    "<< /Type /Font /Subtype /CIDFontType2 /BaseFont {base} /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor {desc} 0 R /DW {dw} /W [{}] /CIDToGIDMap /Identity >>",
                    widths.trim_end()
                ),
            );
            let flags = if font.fixed { 32 + 1 } else { 32 };
            let [x0, y0, x1, y1] = font.bbox.map(|v| font.scaled(v));
            w.obj(
                desc,
                &format!(
                    "<< /Type /FontDescriptor /FontName {base} /Flags {flags} /FontBBox [{x0} {y0} {x1} {y1}] /ItalicAngle 0 /Ascent {} /Descent {} /CapHeight {} /StemV 80 /FontFile2 {file} 0 R >>",
                    font.scaled(font.ascent),
                    font.scaled(font.descent),
                    font.scaled(font.cap_height)
                ),
            );
            w.stream(file, &format!("/Length1 {}", font.data.len()), &font.data);
            w.stream(tounicode, "", to_unicode(&f.used).as_bytes());
        }
        for (k, &(page, contents)) in page_ids.iter().enumerate() {
            w.obj(page, &format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] /Resources {resources} /Contents {contents} 0 R >>", num(self.width), num(self.height)));
            w.stream(contents, "", self.pages[k].as_bytes());
        }
        w.finish(next)
    }
}

/// The CMap from glyph ids back to text.
fn to_unicode(used: &BTreeMap<u16, char>) -> String {
    let mut s = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n/CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
    );
    let entries: Vec<(&u16, &char)> = used.iter().collect();
    for chunk in entries.chunks(100) {
        let _ = writeln!(s, "{} beginbfchar", chunk.len());
        for (g, c) in chunk {
            let mut buf = [0u16; 2];
            let hex: String = c.encode_utf16(&mut buf).iter().map(|u| format!("{u:04X}")).collect();
            let _ = writeln!(s, "<{g:04X}> <{hex}>");
        }
        s.push_str("endbfchar\n");
    }
    s.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    s
}

#[derive(Default)]
struct Writer {
    out: Vec<u8>,
    offsets: BTreeMap<usize, usize>,
}

impl Writer {
    fn obj(&mut self, id: usize, body: &str) {
        self.offsets.insert(id, self.out.len());
        let _ = write!(self.out, "{id} 0 obj\n{body}\nendobj\n");
    }

    fn stream(&mut self, id: usize, extra: &str, data: &[u8]) {
        let mut z = ZlibEncoder::new(Vec::new(), Compression::default());
        let packed = z.write_all(data).and_then(|()| z.finish()).ok();
        self.offsets.insert(id, self.out.len());
        let (bytes, filter) = match &packed {
            Some(p) => (p.as_slice(), " /Filter /FlateDecode"),
            None => (data, ""),
        };
        let _ = write!(self.out, "{id} 0 obj\n<< /Length {}{filter} {extra} >>\nstream\n", bytes.len());
        self.out.extend_from_slice(bytes);
        self.out.extend_from_slice(b"\nendstream\nendobj\n");
    }

    fn finish(mut self, count: usize) -> Vec<u8> {
        let xref = self.out.len();
        let _ = write!(self.out, "xref\n0 {count}\n0000000000 65535 f \n");
        for id in 1..count {
            let off = self.offsets.get(&id).copied().unwrap_or(0);
            let _ = writeln!(self.out, "{off:010} 00000 n ");
        }
        let _ = write!(self.out, "trailer\n<< /Size {count} /Root 1 0 R /Info 3 0 R >>\nstartxref\n{xref}\n%%EOF\n");
        self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_fonts_parse_and_map_chars() {
        let (mono, sans) = bundled_fonts();
        let m = mono.glyph('M').unwrap();
        let i = mono.glyph('i').unwrap();
        assert_eq!(mono.advance(m), mono.advance(i), "Hack is monospace");
        assert!(mono.fixed);
        assert!(mono.glyph('é').is_some() && mono.glyph('λ').is_some());
        assert!(sans.advance(sans.glyph('M').unwrap()) > sans.advance(sans.glyph('i').unwrap()));
        assert_eq!(mono.glyph('\u{10FFFF}'), None);
    }

    #[test]
    fn writes_a_well_formed_file() {
        let (mono, sans) = bundled_fonts();
        let mut pdf = Pdf::new(612.0, 792.0, vec![mono, sans]);
        let p = pdf.add_page();
        pdf.fill_rect(p, 10.0, 10.0, 100.0, 20.0, [0.9, 0.9, 0.9]);
        let end = pdf.text(p, 72.0, 72.0, 0, 10.0, [0.0, 0.0, 0.0], Style { bold: true, ..Default::default() }, "Hello, wörld", Some(6.0));
        assert!((end - (72.0 + 12.0 * 6.0)).abs() < 0.01);
        pdf.hline(p, 72.0, 300.0, 80.0, 0.5, [0.5, 0.5, 0.5]);
        pdf.add_page();
        assert_eq!(pdf.page_count(), 2);
        let bytes = pdf.finish("Test ✓");
        let text = String::from_utf8_lossy(&bytes);
        assert!(bytes.starts_with(b"%PDF-1.7"));
        assert!(text.ends_with("%%EOF\n"));
        assert!(text.contains("/Count 2"));
        assert!(!text.contains("/F1 "), "unused fonts aren't embedded");
        // Every xref offset points at its object (offsets are bytes, and the
        // streams are binary, so search the bytes).
        let xref = bytes.windows(6).rposition(|w| w == b"\nxref\n").unwrap() + 1;
        let table = String::from_utf8(bytes[xref..].to_vec()).unwrap();
        let count: usize = table.lines().nth(1).unwrap().split(' ').nth(1).unwrap().parse().unwrap();
        for (id, line) in table.lines().skip(3).take(count - 1).enumerate() {
            let off: usize = line[..10].parse().unwrap();
            assert!(bytes[off..].starts_with(format!("{} 0 obj", id + 1).as_bytes()), "object {}", id + 1);
        }
        let start: usize = table.lines().rev().nth(1).unwrap().parse().unwrap();
        assert_eq!(start, xref);
    }
}
