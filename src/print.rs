//! Printing: a file (or its selection) as syntax-colored, line-numbered
//! pages, or a Markdown file as its rendered preview, written as PDF (see
//! `pdf`) or as a print-ready HTML page. Always on a light background,
//! whatever the editor's color scheme; "in color" keeps the syntax colors,
//! otherwise text is black.

use crate::markdown::{self, Block, Inline};
use crate::pdf::{self, Pdf, Rgb, Style};
use egui::Color32;
use std::ops::Range;

/// The color scheme pages are highlighted with.
pub const PRINT_THEME: &str = "GitHub Light";

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    pub color: bool,
    pub line_numbers: bool,
    /// Code font size in points.
    pub font_size: f32,
    pub paper: Paper,
    pub tab_width: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self { color: true, line_numbers: true, font_size: 8.5, paper: Paper::from_locale(), tab_width: 4 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Paper {
    Letter,
    A4,
}

impl Paper {
    /// US Letter where it is the norm (US, Canada, Mexico, the Philippines…), else A4.
    pub fn from_locale() -> Self {
        let lang = ["LC_PAPER", "LC_ALL", "LANG"].iter().find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty())).unwrap_or_default();
        let region = lang.split(['.', '@']).next().unwrap_or("").rsplit(['_', '-']).next().unwrap_or("").to_ascii_uppercase();
        if ["US", "CA", "MX", "PH", "CL", "CO", "VE", "GT", "PR"].contains(&region.as_str()) { Paper::Letter } else { Paper::A4 }
    }

    /// Width and height in points.
    pub fn size(self) -> (f32, f32) {
        match self {
            Paper::Letter => (612.0, 792.0),
            Paper::A4 => (595.28, 841.89),
        }
    }

    pub fn css(self) -> &'static str {
        match self {
            Paper::Letter => "letter",
            Paper::A4 => "A4",
        }
    }
}

/// One source line with its highlighting: byte ranges and colors.
#[derive(Clone, Debug, Default)]
pub struct Line {
    pub text: String,
    pub spans: Vec<(Range<usize>, Color32, bool)>,
}

/// What to print.
pub struct Source {
    /// Header title: the file's name (or path).
    pub title: String,
    /// Header date and time.
    pub date: String,
    pub lines: Vec<Line>,
    /// 1-based number of the first line (a selection starts mid-file).
    pub first_line: usize,
}

const MARGIN: f32 = 42.0;
const INK: Rgb = [0.12, 0.14, 0.16];
const DIM: Rgb = [0.42, 0.45, 0.49];
const FAINT: Rgb = [0.82, 0.84, 0.87];
const CODE_BG: Rgb = [0.965, 0.973, 0.98];
const LINK: Rgb = [0.04, 0.41, 0.85];

fn color_of(c: Color32, opts: &Options) -> Rgb {
    if opts.color { pdf::rgb(c) } else { INK }
}

/// Page furniture: title and date at the top, "Page n of N" at the bottom.
fn furniture(pdf: &mut Pdf, page: usize, of: usize, title: &str, date: &str) {
    let (w, h) = (pdf.width, pdf.height);
    pdf.text(page, MARGIN, MARGIN, 1, 9.0, INK, Style { bold: true, ..Default::default() }, title, None);
    let dw = pdf.width(1, 8.0, date);
    pdf.text(page, w - MARGIN - dw, MARGIN, 1, 8.0, DIM, Style::default(), date, None);
    pdf.hline(page, MARGIN, w - MARGIN, MARGIN + 6.0, 0.5, FAINT);
    let n = format!("Page {} of {of}", page + 1);
    let nw = pdf.width(1, 8.0, &n);
    pdf.text(page, (w - nw) / 2.0, h - MARGIN + 14.0, 1, 8.0, DIM, Style::default(), &n, None);
}

/// Top and bottom of the text area.
fn body(pdf: &Pdf) -> (f32, f32) {
    (MARGIN + 22.0, pdf.height - MARGIN)
}

/// Code as PDF pages: line numbers in a gutter, long lines wrapped onto
/// continuation rows, colors from the highlighting.
pub fn code_pdf(src: &Source, opts: &Options) -> Vec<u8> {
    let (mono, sans) = pdf::bundled_fonts();
    let (w, h) = opts.paper.size();
    let size = opts.font_size;
    let cell = mono.advance(mono.glyph('M').unwrap_or(0)) * size / 1000.0;
    let row_h = (size * 1.32).round_ties_even().max(size + 1.0);
    let mut pdf = Pdf::new(w, h, vec![mono, sans]);
    let last = src.first_line + src.lines.len().saturating_sub(1);
    let digits = if opts.line_numbers { last.to_string().len() } else { 0 };
    let gutter = if opts.line_numbers { (digits + 2) as f32 * cell } else { 0.0 };
    let cols = (((w - 2.0 * MARGIN - gutter) / cell).floor() as usize).max(8);
    let (top, bottom) = body(&pdf);
    let per_page = (((bottom - top) / row_h).floor() as usize).max(1);

    // Rows: (line index, char range of the expanded line) — laid out first
    // so the footer can say how many pages there are.
    let mut rows: Vec<(usize, Vec<Run>)> = Vec::new();
    for (i, line) in src.lines.iter().enumerate() {
        let cells = expand_styled(line, opts);
        if cells.is_empty() {
            rows.push((i, Vec::new()));
            continue;
        }
        for chunk in cells.chunks(cols) {
            rows.push((i, runs(chunk)));
        }
    }
    let pages = rows.len().div_ceil(per_page).max(1);
    for p in 0..pages {
        let page = pdf.add_page();
        furniture(&mut pdf, page, pages, &src.title, &src.date);
        let mut prev_line = if p == 0 { None } else { Some(rows[p * per_page - 1].0) };
        for (k, (line, pieces)) in rows.iter().skip(p * per_page).take(per_page).enumerate() {
            let y = top + k as f32 * row_h + size;
            if opts.line_numbers && prev_line != Some(*line) {
                let n = (src.first_line + line).to_string();
                let x = MARGIN + (digits - n.len()) as f32 * cell;
                pdf.text(page, x, y, 0, size, DIM, Style::default(), &n, Some(cell));
            }
            prev_line = Some(*line);
            let mut x = MARGIN + gutter;
            for (text, color, italic) in pieces {
                x = pdf.text(page, x, y, 0, size, *color, Style { italic: *italic, ..Default::default() }, text, Some(cell));
            }
        }
    }
    pdf.finish(&src.title)
}

/// Each char of the line (tabs expanded) with its color and slant.
fn expand_styled(line: &Line, opts: &Options) -> Vec<(char, Rgb, bool)> {
    let tab = opts.tab_width.max(1);
    let mut out = Vec::with_capacity(line.text.len());
    let mut span = 0;
    for (b, c) in line.text.char_indices() {
        while span < line.spans.len() && line.spans[span].0.end <= b {
            span += 1;
        }
        let (color, italic) = match line.spans.get(span).filter(|s| s.0.start <= b) {
            Some((_, col, it)) => (color_of(*col, opts), *it && opts.color),
            None => (INK, false),
        };
        if c == '\t' {
            let n = tab - out.len() % tab;
            out.extend(std::iter::repeat_n((' ', color, false), n));
        } else if !c.is_control() {
            out.push((c, color, italic));
        }
    }
    out
}

/// Consecutive chars with the same look, joined.
/// Text drawn in one color, maybe slanted.
type Run = (String, Rgb, bool);

fn runs(cells: &[(char, Rgb, bool)]) -> Vec<Run> {
    let mut out: Vec<Run> = Vec::new();
    for &(c, color, italic) in cells {
        match out.last_mut() {
            Some((s, col, it)) if *col == color && *it == italic => s.push(c),
            _ => out.push((c.to_string(), color, italic)),
        }
    }
    out
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn hex(c: Color32) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b())
}

const PRINT_CSS: &str = "\
:root{color-scheme:light}
*{-webkit-print-color-adjust:exact;print-color-adjust:exact}
body{margin:0;background:#fff;color:#1f2328;font:12px/1.45 ui-monospace,SFMono-Regular,Menlo,Consolas,\"Liberation Mono\",monospace}
header{display:flex;justify-content:space-between;gap:16px;align-items:baseline;border-bottom:1px solid #d1d9e0;padding:0 0 4px;margin:0 0 8px;font:11px/1.4 -apple-system,BlinkMacSystemFont,\"Segoe UI\",Helvetica,Arial,sans-serif;color:#59636e}
header strong{color:#1f2328;font-size:12px}
table{border-collapse:collapse;width:100%}
td{padding:0;vertical-align:top}
td.n{color:#8c959f;text-align:right;padding-right:1.5em;white-space:nowrap;user-select:none;width:1%}
td.l{white-space:pre-wrap;overflow-wrap:anywhere;word-break:break-all}
tr{break-inside:avoid}
@media screen{body{max-width:110ch;margin:24px auto;padding:0 16px}}
";

/// Code as a standalone HTML page laid out for printing.
pub fn code_html(src: &Source, opts: &Options) -> String {
    let mut rows = String::with_capacity(src.lines.iter().map(|l| l.text.len() * 2 + 40).sum());
    for (i, line) in src.lines.iter().enumerate() {
        rows.push_str("<tr>");
        if opts.line_numbers {
            rows.push_str(&format!("<td class=\"n\">{}</td>", src.first_line + i));
        }
        rows.push_str("<td class=\"l\">");
        let mut at = 0;
        let text = &line.text;
        if opts.color {
            for (r, c, italic) in &line.spans {
                if r.start < at || r.end > text.len() || !text.is_char_boundary(r.start) || !text.is_char_boundary(r.end) {
                    continue;
                }
                rows.push_str(&esc(&text[at..r.start]));
                let style = if *italic { ";font-style:italic" } else { "" };
                rows.push_str(&format!("<span style=\"color:{}{style}\">{}</span>", hex(*c), esc(&text[r.clone()])));
                at = r.end;
            }
        }
        rows.push_str(&esc(&text[at..]));
        if text.is_empty() {
            rows.push('\u{200B}');
        }
        rows.push_str("</td></tr>\n");
    }
    let tab = opts.tab_width.max(1);
    format!(
        "<!DOCTYPE html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n<title>{t}</title>\n<style>\n@page{{size:{paper};margin:15mm 12mm}}\n{PRINT_CSS}body{{tab-size:{tab}}}\n</style>\n</head>\n<body>\n<header><strong>{t}</strong><span>{d}</span></header>\n<table>\n{rows}</table>\n</body>\n</html>\n",
        t = esc(&src.title),
        d = esc(&src.date),
        paper = opts.paper.css(),
    )
}

/// The rendered Markdown page for printing: the export page, kept light.
pub fn markdown_html(md: &str, title: &str, opts: &Options) -> String {
    let page = markdown::to_html_document(md, title);
    let extra = format!("@page{{size:{};margin:18mm 16mm}}\n:root{{color-scheme:light}}\n*{{-webkit-print-color-adjust:exact;print-color-adjust:exact}}\n@media print{{main{{max-width:none!important;margin:0!important;padding:0!important}}}}\n</style>", opts.paper.css());
    page.replacen("@media (prefers-color-scheme:dark)", "@media not all", 1).replacen("</style>", &extra, 1)
}

// ---- Markdown as PDF ---------------------------------------------------------------

const SANS: usize = 1;
const MONO: usize = 0;
const TEXT_SIZE: f32 = 10.5;

/// Lays blocks out top to bottom across pages.
struct Flow {
    pdf: Pdf,
    page: usize,
    y: f32,
    top: f32,
    bottom: f32,
    left: f32,
    right: f32,
    color: bool,
    /// Inside a tight list item: paragraphs get no space around them.
    tight: bool,
    /// Where the next text line lands (page, baseline), recorded once after
    /// being cleared: list markers sit on their item's first line.
    first_line: Option<Option<(usize, f32)>>,
}

/// A word (or space) with its look.
#[derive(Clone)]
struct Piece {
    text: String,
    font: usize,
    size: f32,
    color: Rgb,
    style: Style,
    /// Inline code: drawn on a tint.
    code: bool,
}

impl Flow {
    fn new(opts: &Options) -> Self {
        let (mono, sans) = pdf::bundled_fonts();
        let (w, h) = opts.paper.size();
        let mut pdf = Pdf::new(w, h, vec![mono, sans]);
        let page = pdf.add_page();
        let (top, bottom) = (MARGIN + 6.0, h - MARGIN);
        Self { pdf, page, y: top, top, bottom, left: MARGIN + 14.0, right: w - MARGIN - 14.0, color: opts.color, tight: false, first_line: None }
    }

    fn new_page(&mut self) {
        self.page = self.pdf.add_page();
        self.y = self.top;
    }

    /// Room for `h` more points, or a new page.
    fn need(&mut self, h: f32) {
        if self.y + h > self.bottom && self.y > self.top {
            self.new_page();
        }
    }

    fn gap(&mut self, h: f32) {
        if self.y > self.top {
            self.y += h;
        }
    }

    fn tint(&self, c: Rgb) -> Rgb {
        if self.color { c } else { INK }
    }

    /// Wrap `pieces` into lines between `left` and `right`, drawing them;
    /// `bar` draws a quote bar at that x on every line.
    fn paragraph(&mut self, pieces: &[Piece], left: f32, line_h: f32, bars: &[f32]) {
        let right = self.right;
        let mut line: Vec<(Piece, f32)> = Vec::new();
        let mut x = left;
        let flush = |flow: &mut Flow, line: &mut Vec<(Piece, f32)>| {
            while line.last().is_some_and(|(p, _)| p.text.trim().is_empty()) {
                line.pop();
            }
            flow.need(line_h);
            let base = flow.y + line_h * 0.78;
            if let Some(slot @ None) = &mut flow.first_line {
                *slot = Some((flow.page, base));
            }
            for &b in bars {
                flow.pdf.fill_rect(flow.page, b, flow.y, 2.5, line_h, FAINT);
            }
            for (p, px) in line.drain(..) {
                if p.code {
                    let w = flow.pdf.width(p.font, p.size, &p.text);
                    flow.pdf.fill_rect(flow.page, px - 1.5, flow.y + line_h * 0.12, w + 3.0, line_h * 0.82, [0.93, 0.94, 0.95]);
                }
                flow.pdf.text(flow.page, px, base, p.font, p.size, p.color, p.style, &p.text, None);
            }
            flow.y += line_h;
        };
        for p in pieces {
            for part in split_keep(&p.text) {
                if part == "\n" {
                    flush(self, &mut line);
                    x = left;
                    continue;
                }
                let piece = Piece { text: part.to_owned(), ..p.clone() };
                let w = self.pdf.width(piece.font, piece.size, &piece.text);
                let space = part.trim().is_empty();
                if x + w > right && !line.is_empty() && !space {
                    flush(self, &mut line);
                    x = left;
                }
                if space && line.is_empty() {
                    continue;
                }
                if w > right - left {
                    // A word wider than the line: break it by characters.
                    let mut chunk = String::new();
                    for c in piece.text.chars() {
                        chunk.push(c);
                        if x + self.pdf.width(piece.font, piece.size, &chunk) > right && chunk.chars().count() > 1 {
                            let last = chunk.pop().unwrap_or(' ');
                            line.push((Piece { text: std::mem::take(&mut chunk), ..piece.clone() }, x));
                            flush(self, &mut line);
                            x = left;
                            chunk.push(last);
                        }
                    }
                    let cw = self.pdf.width(piece.font, piece.size, &chunk);
                    line.push((Piece { text: chunk, ..piece.clone() }, x));
                    x += cw;
                    continue;
                }
                line.push((piece, x));
                x += w;
            }
        }
        if !line.is_empty() {
            flush(self, &mut line);
        }
    }

    fn inline_pieces(&self, inline: &Inline, size: f32, color: Rgb, bold: bool) -> Vec<Piece> {
        let mut out = Vec::new();
        for run in &inline.runs {
            let Some(text) = inline.text.get(run.range.clone()) else { continue };
            let s = run.style;
            let link = run.link.is_some();
            let (font, sz) = if s.code { (MONO, size * 0.9) } else { (SANS, size) };
            let col = if link { self.tint(LINK) } else if s.html { DIM } else { color };
            let style = Style { bold: bold || s.bold, italic: s.italic || s.image, underline: link, strike: s.strike };
            out.push(Piece { text: text.to_owned(), font, size: if s.note { sz * 0.75 } else { sz }, color: col, style, code: s.code });
        }
        out
    }

    fn blocks(&mut self, blocks: &[Block], left: f32, bars: &[f32]) {
        for b in blocks {
            self.block(b, left, bars);
        }
    }

    fn block(&mut self, b: &Block, left: f32, bars: &[f32]) {
        let line_h = TEXT_SIZE * 1.5;
        match b {
            Block::Heading { level, text, .. } => {
                let size = match level {
                    1 => 20.0,
                    2 => 16.0,
                    3 => 13.5,
                    4 => 12.0,
                    _ => 11.0,
                };
                self.gap(size * 0.9);
                self.need(size * 1.3 + line_h * 2.0);
                let pieces = self.inline_pieces(text, size, INK, true);
                self.paragraph(&pieces, left, size * 1.3, bars);
                if *level <= 2 {
                    self.pdf.hline(self.page, left, self.right, self.y + 2.0, 0.6, FAINT);
                    self.y += 6.0;
                }
                self.y += 3.0;
            }
            Block::Paragraph(text) => {
                let space = if self.tight { 0.0 } else { 4.0 };
                self.gap(space);
                let pieces = self.inline_pieces(text, TEXT_SIZE, INK, false);
                self.paragraph(&pieces, left, line_h, bars);
                self.y += space;
            }
            Block::Code { lang, code } => self.code_block(lang, code, left, bars),
            Block::Quote { kind, blocks } => {
                self.gap(4.0);
                let mut bars = bars.to_vec();
                bars.push(left);
                if let Some(k) = kind {
                    let label = Piece { text: (*k).to_owned(), font: SANS, size: TEXT_SIZE, color: self.tint(LINK), style: Style { bold: true, ..Default::default() }, code: false };
                    self.paragraph(&[label], left + 12.0, line_h, &bars);
                }
                self.blocks(blocks, left + 12.0, &bars);
                self.y += 2.0;
            }
            Block::List { start, items } => {
                self.gap(2.0);
                for (n, item) in items.iter().enumerate() {
                    let marker = match (item.task, start) {
                        (Some(true), _) => "[x]".to_owned(),
                        (Some(false), _) => "[ ]".to_owned(),
                        (None, Some(s)) => format!("{}.", s + n as u64),
                        (None, None) => "•".to_owned(),
                    };
                    self.need(line_h);
                    let font = if item.task.is_some() { MONO } else { SANS };
                    let mw = self.pdf.width(font, TEXT_SIZE, &marker);
                    let indent = left + (mw + 8.0).max(18.0);
                    let (outer_tight, outer_first) = (self.tight, self.first_line.take());
                    self.tight = !item.loose;
                    self.first_line = Some(None);
                    let start = (self.page, self.y + line_h * 0.78);
                    self.blocks(&item.blocks, indent, bars);
                    if item.blocks.is_empty() {
                        self.y += line_h;
                    }
                    let (page, base) = self.first_line.flatten().unwrap_or(start);
                    self.pdf.text(page, indent - 6.0 - mw, base, font, TEXT_SIZE, DIM, Style::default(), &marker, None);
                    (self.tight, self.first_line) = (outer_tight, outer_first);
                    if let Some(slot @ None) = &mut self.first_line {
                        *slot = Some((page, base));
                    }
                    self.y += if item.loose { 6.0 } else { 2.0 };
                }
            }
            Block::Table { head, rows, .. } => self.table(head, rows, left, bars),
            Block::Rule => {
                self.gap(6.0);
                self.need(8.0);
                self.pdf.hline(self.page, left, self.right, self.y + 4.0, 1.0, FAINT);
                self.y += 10.0;
            }
            Block::Html(html) => {
                let p = Piece { text: html.trim_end().to_owned(), font: MONO, size: TEXT_SIZE * 0.85, color: DIM, style: Style::default(), code: false };
                self.paragraph(&[p], left, line_h * 0.9, bars);
            }
            Block::Footnote { label, blocks } => {
                self.gap(2.0);
                let p = Piece { text: format!("[{label}]"), font: SANS, size: TEXT_SIZE * 0.85, color: DIM, style: Style::default(), code: false };
                self.paragraph(&[p], left, line_h, bars);
                self.blocks(blocks, left + 14.0, bars);
            }
        }
    }

    fn code_block(&mut self, lang: &str, code: &str, left: f32, bars: &[f32]) {
        self.gap(4.0);
        let size = TEXT_SIZE * 0.85;
        let row_h = size * 1.4;
        let (mono, _) = pdf::bundled_fonts();
        let cell = mono.advance(mono.glyph('M').unwrap_or(0)) * size / 1000.0;
        let cols = (((self.right - left - 16.0) / cell).floor() as usize).max(8);
        let spans = crate::highlight::highlight_snippet(code, lang, PRINT_THEME).unwrap_or_default();
        let opts = Options { color: self.color, ..Default::default() };
        let code = code.strip_suffix('\n').unwrap_or(code);
        let mut offset = 0;
        let mut rows: Vec<Vec<Run>> = Vec::new();
        for text in code.split('\n') {
            let line_spans = spans
                .iter()
                .filter(|s| s.range.end > offset && s.range.start < offset + text.len())
                .map(|s| (s.range.start.saturating_sub(offset)..(s.range.end - offset).min(text.len()), s.color, s.italic))
                .collect();
            let cells = expand_styled(&Line { text: text.to_owned(), spans: line_spans }, &opts);
            if cells.is_empty() {
                rows.push(Vec::new());
            }
            rows.extend(cells.chunks(cols).map(runs));
            offset += text.len() + 1;
        }
        self.y += 4.0;
        for (k, row) in rows.iter().enumerate() {
            self.need(row_h);
            let pad_top = if k == 0 || self.y == self.top { 6.0 } else { 0.0 };
            let pad_bottom = if k + 1 == rows.len() { 6.0 } else { 0.0 };
            self.pdf.fill_rect(self.page, left, self.y - pad_top, self.right - left, row_h + pad_top + pad_bottom, CODE_BG);
            for &b in bars {
                self.pdf.fill_rect(self.page, b, self.y - pad_top, 2.5, row_h + pad_top + pad_bottom, FAINT);
            }
            let mut x = left + 8.0;
            for (text, color, italic) in row {
                x = self.pdf.text(self.page, x, self.y + size, MONO, size, *color, Style { italic: *italic, ..Default::default() }, text, Some(cell));
            }
            self.y += row_h;
        }
        self.y += 10.0;
    }

    fn table(&mut self, head: &[Inline], rows: &[Vec<Inline>], left: f32, bars: &[f32]) {
        self.gap(6.0);
        let n = head.len().max(rows.iter().map(Vec::len).max().unwrap_or(0));
        if n == 0 {
            return;
        }
        let size = TEXT_SIZE * 0.92;
        let line_h = size * 1.45;
        let mut widths = vec![30.0f32; n];
        for (r, row) in std::iter::once(head).chain(rows.iter().map(Vec::as_slice)).enumerate() {
            for (i, c) in row.iter().enumerate() {
                let natural: f32 = self.inline_pieces(c, size, INK, r == 0).iter().map(|p| self.pdf.width(p.font, p.size, &p.text)).sum();
                widths[i] = widths[i].max(natural + 14.0);
            }
        }
        let avail = self.right - left;
        let total: f32 = widths.iter().sum();
        if total > avail {
            for w in &mut widths {
                *w *= avail / total;
            }
        }
        let width: f32 = widths.iter().sum();
        let draw_row = |flow: &mut Flow, row: &[Inline], bold: bool| {
            // Lay each cell out in its own column, then advance by the tallest.
            let (y0, page0) = (flow.y, flow.page);
            let (saved_right, mut x) = (flow.right, left);
            let mut tallest = line_h;
            for (i, w) in widths.iter().enumerate() {
                flow.y = y0;
                flow.page = page0;
                flow.right = x + w - 6.0;
                if let Some(cell) = row.get(i) {
                    let pieces = flow.inline_pieces(cell, size, INK, bold);
                    flow.paragraph(&pieces, x + 6.0, line_h, &[]);
                }
                tallest = tallest.max(if flow.page == page0 { flow.y - y0 } else { line_h });
                x += w;
            }
            flow.right = saved_right;
            flow.page = page0;
            flow.y = y0 + tallest;
            flow.pdf.hline(flow.page, left, left + width, flow.y + 1.0, if bold { 0.9 } else { 0.4 }, FAINT);
            flow.y += 3.0;
        };
        let _ = bars;
        self.need(line_h * 2.0);
        draw_row(self, head, true);
        for row in rows {
            self.need(line_h + 4.0);
            draw_row(self, row, false);
        }
        self.y += 6.0;
    }
}

/// Words, the spaces between them and line breaks, as separate pieces.
fn split_keep(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut prev_space: Option<bool> = None;
    for (i, c) in s.char_indices() {
        if c == '\n' {
            if start < i {
                out.push(&s[start..i]);
            }
            out.push("\n");
            start = i + 1;
            prev_space = None;
            continue;
        }
        let space = c == ' ' || c == '\t';
        if prev_space.is_some_and(|p| p != space) {
            out.push(&s[start..i]);
            start = i;
        }
        prev_space = Some(space);
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// A Markdown file as PDF pages, laid out like its preview.
pub fn markdown_pdf(md: &str, title: &str, date: &str, opts: &Options) -> Vec<u8> {
    let mut flow = Flow::new(opts);
    flow.top = MARGIN + 22.0;
    flow.y = flow.top;
    for top in markdown::parse(md) {
        flow.block(&top.block, flow.left, &[]);
    }
    let pages = flow.pdf.page_count();
    for p in 0..pages {
        furniture(&mut flow.pdf, p, pages, title, date);
    }
    flow.pdf.finish(title)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(lines: &[&str]) -> Source {
        Source {
            title: "main.rs".into(),
            date: "2026-10-13 09:30".into(),
            lines: lines.iter().map(|l| Line { text: (*l).into(), spans: vec![(0..l.len().min(2), Color32::from_rgb(0xcf, 0x22, 0x2e), false)] }).collect(),
            first_line: 1,
        }
    }

    #[test]
    fn code_pdf_pages_wrap_and_number() {
        let long = "x".repeat(400);
        let mut lines: Vec<&str> = vec!["fn main() {", "\tprintln!(\"héllo\");", "}", "", &long];
        lines.extend(std::iter::repeat_n("// filler", 150));
        let opts = Options { paper: Paper::Letter, ..Default::default() };
        let bytes = code_pdf(&source(&lines), &opts);
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("/Count 3"), "155 lines plus wrapped rows fill three Letter pages");
        assert!(text.contains("/MediaBox [0 0 612 792]"));
        let a4 = code_pdf(&source(&lines[..3]), &Options { paper: Paper::A4, ..Default::default() });
        assert!(String::from_utf8_lossy(&a4).contains("/MediaBox [0 0 595.28 841.89]"));
    }

    #[test]
    fn code_html_escapes_and_colors() {
        let mut src = source(&["a < b && c", ""]);
        src.first_line = 41;
        let html = code_html(&src, &Options::default());
        assert!(html.contains("<td class=\"n\">41</td>"));
        assert!(html.contains("<span style=\"color:#cf222e\">a </span>&lt; b &amp;&amp; c"));
        assert!(html.contains("@page{size:"));
        let plain = code_html(&src, &Options { color: false, line_numbers: false, ..Default::default() });
        assert!(!plain.contains("<span style") && !plain.contains("class=\"n\""));
    }

    #[test]
    fn markdown_prints_as_rendered_pages() {
        let md = "# Title\n\nSome *emphasis*, **strong** and `code` with a [link](https://x.y).\n\n- one\n- two\n  1. nested\n\n> quoted text\n\n```rust\nfn main() {}\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n---\n\n- [x] done\n";
        let bytes = markdown_pdf(md, "README.md", "today", &Options::default());
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.starts_with("%PDF") && text.contains("/Count 1"));
        let long = format!("# Big\n\n{}", "A paragraph that goes on. ".repeat(2_000));
        let bytes = markdown_pdf(&long, "big.md", "today", &Options::default());
        let pages: usize = String::from_utf8_lossy(&bytes).split("/Count ").nth(1).unwrap().split(' ').next().unwrap().parse().unwrap();
        assert!(pages > 5);
        let html = markdown_html(md, "README", &Options::default());
        assert!(html.contains("<h1>Title</h1>") && html.contains("@page{size:"));
        assert!(!html.contains("prefers-color-scheme:dark"));
    }

    #[test]
    fn words_split_at_spaces_and_breaks() {
        assert_eq!(split_keep("ab  cd\nef"), ["ab", "  ", "cd", "\n", "ef"]);
    }
}
