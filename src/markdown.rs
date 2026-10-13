//! Markdown live preview: pulldown-cmark parses the active document into a
//! small block model, which is drawn with egui in the current color scheme.
//!
//! Parsing happens once per edit (debounced for very large files) and only
//! the blocks inside the viewport are laid out, using measured heights for
//! the rest, so a megabyte of Markdown stays responsive. Also: export as a
//! standalone HTML page.

use crate::document::Document;
use crate::highlight::{self, Highlighter, Span};
use egui::epaint::text::VariationCoords;
use egui::text::{LayoutJob, TextFormat};
use egui::{Align, Color32, CursorIcon, FontFamily, FontId, Galley, Layout, Rect, Sense, Stroke, Ui, UiBuilder, pos2, vec2};
use pulldown_cmark::{Alignment, BlockQuoteKind, CodeBlockKind, Event, HeadingLevel, Options, Tag, TagEnd};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// File extensions treated as Markdown.
pub const EXTENSIONS: &[&str] = &["md", "markdown", "mdown", "mkd", "mkdn"];

pub fn is_markdown(path: Option<&Path>, syntax: &str) -> bool {
    syntax == "Markdown"
        || path
            .and_then(|p| p.extension())
            .and_then(|e| e.to_str())
            .is_some_and(|e| EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

/// GitHub-flavored extensions: tables, ~~strike~~, - [ ] tasks, footnotes, > [!NOTE].
fn options() -> Options {
    Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS | Options::ENABLE_FOOTNOTES | Options::ENABLE_GFM
}

// ---- block model ------------------------------------------------------------------

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
    /// An image placeholder ("🖼 alt").
    pub image: bool,
    /// A footnote reference, drawn small and raised.
    pub note: bool,
    /// Raw inline HTML, drawn dimmed.
    pub html: bool,
}

/// A styled stretch of `Inline::text` (byte range), maybe a link (index into `links`).
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub range: Range<usize>,
    pub style: Style,
    pub link: Option<usize>,
}

/// One paragraph's worth of styled text.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Inline {
    pub text: String,
    pub runs: Vec<Run>,
    pub links: Vec<String>,
}

impl Inline {
    fn push(&mut self, s: &str, style: Style, link: Option<usize>) {
        if s.is_empty() {
            return;
        }
        let start = self.text.len();
        self.text.push_str(s);
        match self.runs.last_mut() {
            Some(r) if r.style == style && r.link == link => r.range.end = self.text.len(),
            _ => self.runs.push(Run { range: start..self.text.len(), style, link }),
        }
    }

    /// The link under byte `at`, if any.
    fn link_at(&self, at: usize) -> Option<&str> {
        let run = self.runs.iter().find(|r| r.range.contains(&at))?;
        run.link.map(|i| self.links[i].as_str())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    /// `Some(done)` for a task list item.
    pub task: Option<bool>,
    /// Paragraphs separated by blank lines (a "loose" list): more spacing.
    pub loose: bool,
    pub blocks: Vec<Block>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Heading { level: u8, text: Inline, slug: String },
    Paragraph(Inline),
    Code { lang: String, code: String },
    /// `kind` is a GitHub alert label ("Note", "Warning", …).
    Quote { kind: Option<&'static str>, blocks: Vec<Block> },
    List { start: Option<u64>, items: Vec<Item> },
    Table { aligns: Vec<Align>, head: Vec<Inline>, rows: Vec<Vec<Inline>> },
    Rule,
    Html(String),
    Footnote { label: String, blocks: Vec<Block> },
}

/// A top-level block with where it starts in the source (for scroll sync)
/// and a hash of its source (to remember its measured height across edits).
#[derive(Clone, Debug)]
pub struct TopBlock {
    pub block: Block,
    pub line: usize,
    hash: u64,
    /// Source size, for a height estimate before the block is first drawn.
    chars: usize,
    lines: usize,
}

/// Parse Markdown into top-level blocks.
pub fn parse(src: &str) -> Vec<TopBlock> {
    let mut b = Builder { it: pulldown_cmark::Parser::new_ext(src, options()).into_offset_iter(), tasks: Vec::new(), paragraphs: false };
    let mut out = Vec::new();
    let (mut line, mut counted) = (0, 0);
    while let Some((ev, range)) = b.it.next() {
        let block = match ev {
            Event::Start(tag) => b.block(tag),
            Event::Rule => Some(Block::Rule),
            Event::Html(s) | Event::Text(s) => html_block(s.to_string()),
            _ => None,
        };
        let Some(block) = block else { continue };
        if range.start >= counted {
            line += memchr_count(&src.as_bytes()[counted..range.start]);
            counted = range.start;
        }
        let source = src.get(range.clone()).unwrap_or("");
        let mut h = std::collections::hash_map::DefaultHasher::new();
        source.hash(&mut h);
        out.push(TopBlock { block, line, hash: h.finish(), chars: source.len(), lines: memchr_count(source.as_bytes()) + 1 });
    }
    out
}

fn memchr_count(bytes: &[u8]) -> usize {
    bytes.iter().filter(|&&b| b == b'\n').count()
}

fn html_block(s: String) -> Option<Block> {
    let t = s.trim();
    (!t.is_empty() && !(t.starts_with("<!--") && t.ends_with("-->"))).then(|| Block::Html(t.to_owned()))
}

/// GitHub-style anchor for a heading: "Hello, World!" → "hello-world".
pub fn slug(text: &str) -> String {
    text.trim()
        .chars()
        .filter_map(|c| match c {
            c if c.is_alphanumeric() => Some(c.to_lowercase().next().unwrap_or(c)),
            '-' | '_' => Some(c),
            c if c.is_whitespace() => Some('-'),
            _ => None,
        })
        .collect()
}

struct Builder<'a> {
    it: pulldown_cmark::OffsetIter<'a, pulldown_cmark::DefaultBrokenLinkCallback>,
    /// Task marker of each open list item, innermost last.
    tasks: Vec<Option<bool>>,
    /// The last `blocks` call saw explicit paragraphs.
    paragraphs: bool,
}

/// Inline styling state while reading a paragraph.
#[derive(Default)]
struct InlineState {
    inline: Inline,
    bold: u32,
    italic: u32,
    strike: u32,
    links: Vec<Option<usize>>,
    /// Text length when the current image started (to fill in a missing alt).
    image: Vec<(usize, String)>,
}

impl InlineState {
    fn style(&self) -> Style {
        Style { bold: self.bold > 0, italic: self.italic > 0, strike: self.strike > 0, image: !self.image.is_empty(), ..Default::default() }
    }

    fn link(&self) -> Option<usize> {
        self.links.iter().rev().find_map(|l| *l)
    }

    fn text(&mut self, s: &str, extra: impl FnOnce(&mut Style)) {
        let mut style = self.style();
        extra(&mut style);
        let link = self.link();
        self.inline.push(s, style, link);
    }

    /// Feed one inline event; false if it isn't one.
    fn event(&mut self, ev: Event) -> bool {
        match ev {
            Event::Text(t) => self.text(&t, |_| {}),
            Event::Code(t) => self.text(&t, |s| s.code = true),
            Event::InlineMath(t) | Event::DisplayMath(t) => self.text(&t, |s| s.code = true),
            Event::SoftBreak => self.text(" ", |_| {}),
            Event::HardBreak => self.text("\n", |_| {}),
            Event::InlineHtml(t) => {
                let t = t.trim();
                if !(t.starts_with("<!--") || t.eq_ignore_ascii_case("<br>") || t.eq_ignore_ascii_case("<br/>") || t.eq_ignore_ascii_case("<br />")) {
                    self.text(t, |s| s.html = true);
                } else if t.to_ascii_lowercase().starts_with("<br") {
                    self.text("\n", |_| {});
                }
            }
            Event::FootnoteReference(label) => self.text(&format!("[{label}]"), |s| s.note = true),
            Event::Start(Tag::Emphasis) => self.italic += 1,
            Event::Start(Tag::Strong) => self.bold += 1,
            Event::Start(Tag::Strikethrough) => self.strike += 1,
            Event::Start(Tag::Superscript | Tag::Subscript) => {}
            Event::Start(Tag::Link { dest_url, .. }) => {
                self.inline.links.push(dest_url.to_string());
                self.links.push(Some(self.inline.links.len() - 1));
            }
            Event::Start(Tag::Image { dest_url, .. }) => {
                self.image.push((self.inline.text.len(), dest_url.to_string()));
                self.text("🖼 ", |_| {});
            }
            Event::End(TagEnd::Emphasis) => self.italic = self.italic.saturating_sub(1),
            Event::End(TagEnd::Strong) => self.bold = self.bold.saturating_sub(1),
            Event::End(TagEnd::Strikethrough) => self.strike = self.strike.saturating_sub(1),
            Event::End(TagEnd::Superscript | TagEnd::Subscript) => {}
            Event::End(TagEnd::Link) => {
                self.links.pop();
            }
            Event::End(TagEnd::Image) => {
                let style = self.style();
                if let Some((start, url)) = self.image.pop()
                    && self.inline.text[start..].trim() == "🖼"
                {
                    let name = url.rsplit('/').next().unwrap_or(&url).to_owned();
                    self.inline.push(&name, style, self.link());
                }
            }
            _ => return false,
        }
        true
    }
}

impl Builder<'_> {
    fn next(&mut self) -> Option<Event<'_>> {
        self.it.next().map(|(e, _)| e)
    }

    /// Read inline content up to `end`.
    fn inline(&mut self, end: TagEnd) -> Inline {
        let mut st = InlineState::default();
        while let Some(ev) = self.next() {
            match ev {
                Event::End(e) if e == end => break,
                Event::TaskListMarker(done) => self.set_task(done),
                ev => {
                    st.event(ev);
                }
            }
        }
        st.inline
    }

    fn set_task(&mut self, done: bool) {
        if let Some(t) = self.tasks.last_mut() {
            *t = Some(done);
        }
    }

    /// Child blocks of a container, up to its end. Loose text (tight list
    /// items) becomes a paragraph.
    fn blocks(&mut self) -> Vec<Block> {
        let mut out = Vec::new();
        let mut loose = InlineState::default();
        let mut paragraphs = false;
        let flush = |loose: &mut InlineState, out: &mut Vec<Block>| {
            let st = std::mem::take(loose);
            if !st.inline.text.is_empty() {
                out.push(Block::Paragraph(st.inline));
            }
        };
        while let Some(ev) = self.it.next().map(|(e, _)| e) {
            match ev {
                Event::TaskListMarker(done) => self.set_task(done),
                Event::Rule => {
                    flush(&mut loose, &mut out);
                    out.push(Block::Rule);
                }
                Event::Html(s) => {
                    flush(&mut loose, &mut out);
                    out.extend(html_block(s.to_string()));
                }
                Event::Start(tag) if is_block(&tag) => {
                    flush(&mut loose, &mut out);
                    paragraphs |= matches!(tag, Tag::Paragraph);
                    out.extend(self.block(tag));
                }
                Event::End(e) if !is_inline_end(&e) => break,
                ev => {
                    loose.event(ev);
                }
            }
        }
        flush(&mut loose, &mut out);
        self.paragraphs = paragraphs;
        out
    }

    fn block(&mut self, tag: Tag) -> Option<Block> {
        Some(match tag {
            Tag::Paragraph => Block::Paragraph(self.inline(TagEnd::Paragraph)),
            Tag::Heading { level, .. } => {
                let text = self.inline(TagEnd::Heading(level));
                let slug = slug(&text.text);
                Block::Heading { level: level_num(level), text, slug }
            }
            Tag::BlockQuote(kind) => Block::Quote { kind: kind.map(alert_label), blocks: self.blocks() },
            Tag::CodeBlock(kind) => {
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => info.split(|c: char| c.is_whitespace() || c == ',' || c == '{').next().unwrap_or("").to_owned(),
                    CodeBlockKind::Indented => String::new(),
                };
                let mut code = String::new();
                while let Some(ev) = self.next() {
                    match ev {
                        Event::Text(t) => code.push_str(&t),
                        Event::End(_) => break,
                        _ => {}
                    }
                }
                if code.ends_with('\n') {
                    code.pop();
                }
                Block::Code { lang, code }
            }
            Tag::HtmlBlock => {
                let mut html = String::new();
                while let Some(ev) = self.next() {
                    match ev {
                        Event::Html(t) | Event::Text(t) => html.push_str(&t),
                        Event::End(_) => break,
                        _ => {}
                    }
                }
                return html_block(html);
            }
            Tag::List(start) => {
                let mut items = Vec::new();
                while let Some(ev) = self.next() {
                    match ev {
                        Event::Start(Tag::Item) => items.push(self.item()),
                        Event::End(TagEnd::List(_)) => break,
                        _ => {}
                    }
                }
                Block::List { start, items }
            }
            Tag::Item => Block::List { start: None, items: vec![self.item()] },
            Tag::FootnoteDefinition(label) => Block::Footnote { label: label.to_string(), blocks: self.blocks() },
            Tag::Table(aligns) => {
                let aligns = aligns
                    .iter()
                    .map(|a| match a {
                        Alignment::Center => Align::Center,
                        Alignment::Right => Align::Max,
                        _ => Align::Min,
                    })
                    .collect();
                let (mut head, mut rows) = (Vec::new(), Vec::new());
                while let Some(ev) = self.next() {
                    match ev {
                        Event::Start(Tag::TableHead) => head = self.cells(TagEnd::TableHead),
                        Event::Start(Tag::TableRow) => {
                            let row = self.cells(TagEnd::TableRow);
                            rows.push(row);
                        }
                        Event::End(TagEnd::Table) => break,
                        _ => {}
                    }
                }
                Block::Table { aligns, head, rows }
            }
            other => {
                // Metadata, definition lists…: skip to the matching end.
                let end = other.to_end();
                while let Some(ev) = self.next() {
                    if matches!(ev, Event::End(e) if e == end) {
                        break;
                    }
                }
                return None;
            }
        })
    }

    fn item(&mut self) -> Item {
        self.tasks.push(None);
        let blocks = self.blocks();
        let task = self.tasks.pop().flatten();
        Item { task, loose: self.paragraphs, blocks }
    }

    fn cells(&mut self, end: TagEnd) -> Vec<Inline> {
        let mut cells = Vec::new();
        while let Some(ev) = self.next() {
            match ev {
                Event::Start(Tag::TableCell) => cells.push(self.inline(TagEnd::TableCell)),
                Event::End(e) if e == end => break,
                _ => {}
            }
        }
        cells
    }
}

fn is_block(tag: &Tag) -> bool {
    !matches!(tag, Tag::Emphasis | Tag::Strong | Tag::Strikethrough | Tag::Superscript | Tag::Subscript | Tag::Link { .. } | Tag::Image { .. })
}

fn is_inline_end(end: &TagEnd) -> bool {
    matches!(end, TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Superscript | TagEnd::Subscript | TagEnd::Link | TagEnd::Image)
}

fn level_num(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn alert_label(kind: BlockQuoteKind) -> &'static str {
    match kind {
        BlockQuoteKind::Note => "Note",
        BlockQuoteKind::Tip => "Tip",
        BlockQuoteKind::Important => "Important",
        BlockQuoteKind::Warning => "Warning",
        BlockQuoteKind::Caution => "Caution",
    }
}

// ---- HTML export ------------------------------------------------------------------

const EXPORT_CSS: &str = "\
:root{color-scheme:light dark;--fg:#1f2328;--bg:#fff;--dim:#59636e;--rule:#d1d9e0;--code:#f6f8fa;--link:#0969da}
@media (prefers-color-scheme:dark){:root{--fg:#e6edf3;--bg:#0d1117;--dim:#9198a1;--rule:#3d444d;--code:#151b23;--link:#4493f8}}
body{margin:0;background:var(--bg);color:var(--fg);font:16px/1.6 -apple-system,BlinkMacSystemFont,\"Segoe UI\",Helvetica,Arial,sans-serif}
main{max-width:860px;margin:0 auto;padding:32px 24px 64px}
h1,h2{padding-bottom:.3em;border-bottom:1px solid var(--rule)}
h1,h2,h3,h4,h5,h6{margin:1.5em 0 .6em;line-height:1.25}
a{color:var(--link);text-decoration:none}a:hover{text-decoration:underline}
code,pre{font:85% ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;background:var(--code);border-radius:6px}
code{padding:.2em .4em}pre{padding:16px;overflow:auto;line-height:1.45}pre code{padding:0;background:none;font-size:100%}
blockquote{margin:0 0 16px;padding:0 1em;color:var(--dim);border-left:.25em solid var(--rule)}
table{border-collapse:collapse;margin:0 0 16px;display:block;overflow:auto}
th,td{border:1px solid var(--rule);padding:6px 13px}th{background:var(--code)}tr:nth-child(2n) td{background:color-mix(in srgb,var(--code) 50%,transparent)}
hr{border:0;height:1px;background:var(--rule);margin:24px 0}img{max-width:100%}
li.task-list-item,ul:has(>li>input[type=checkbox]){list-style:none}input[type=checkbox]{margin:0 .4em 0 -1.4em}
.footnote-definition{font-size:90%;color:var(--dim)}
.markdown-alert{border-left-color:var(--link)}";

/// Markdown `src` as HTML body markup (no page around it).
pub fn to_html_fragment(src: &str) -> String {
    let mut body = String::new();
    pulldown_cmark::html::push_html(&mut body, pulldown_cmark::Parser::new_ext(src, options()));
    body
}

/// A standalone HTML page for Markdown `src`. The title is the first heading,
/// else `fallback_title`.
pub fn to_html_document(src: &str, fallback_title: &str) -> String {
    let body = to_html_fragment(src);
    let title = parse(src)
        .into_iter()
        .find_map(|b| match b.block {
            Block::Heading { text, .. } => Some(text.text),
            _ => None,
        })
        .unwrap_or_else(|| fallback_title.to_owned());
    let title = title.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    format!(
        "<!DOCTYPE html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>{title}</title>\n<style>\n{EXPORT_CSS}\n</style>\n</head>\n<body>\n<main>\n{body}</main>\n</body>\n</html>\n"
    )
}

// ---- drawing ----------------------------------------------------------------------

/// What the preview asks the window to do.
#[derive(Debug, PartialEq)]
pub enum Action {
    /// A relative link to a local text file: open it in a tab.
    Open(PathBuf),
    Status(String),
}

/// Colors taken from the syntect theme.
#[derive(Clone)]
struct Palette {
    bg: Color32,
    fg: Color32,
    strong: Color32,
    dim: Color32,
    heading: Color32,
    link: Color32,
    code_fg: Color32,
    code_bg: Color32,
    rule: Color32,
    accent: Color32,
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

impl Palette {
    fn new(hl: &Highlighter) -> Self {
        let theme = hl.theme_name();
        let (bg, fg) = (hl.background(), hl.foreground());
        let accent = hl.caret();
        let scope = |s: &str| highlight::scope_color(theme, s);
        let extreme = if hl.is_light() { Color32::BLACK } else { Color32::WHITE };
        Self {
            bg,
            fg,
            strong: mix(fg, extreme, 0.4),
            dim: mix(fg, bg, 0.35),
            heading: scope("markup.heading").or_else(|| scope("keyword")).unwrap_or(accent),
            link: scope("markup.underline.link").or_else(|| scope("entity.name.function")).unwrap_or(accent),
            code_fg: scope("markup.raw.inline").or_else(|| scope("string")).unwrap_or(fg),
            code_bg: mix(bg, fg, 0.07),
            rule: mix(bg, fg, 0.18),
            accent,
        }
    }
}

/// Per-frame drawing state.
struct Draw<'a> {
    pal: &'a Palette,
    family: FontFamily,
    /// The family is a variable font: bold is a real weight, not just color.
    variable: bool,
    size: f32,
    theme: &'a str,
    code: &'a mut HashMap<u64, Option<Arc<Vec<Span>>>>,
    clicked: Option<String>,
    /// Footnote and table sizes.
    small: f32,
}

impl Draw<'_> {
    fn format(&self, style: Style, size: f32, color: Color32, bold: bool) -> TextFormat {
        let bold = bold || style.bold;
        let mut f = TextFormat {
            font_id: if style.code { FontId::monospace(size * 0.88) } else { FontId::new(size, self.family.clone()) },
            // Plain text gets brighter when bold; dimmed or colored text keeps its color.
            color: if bold && color == self.pal.fg { self.pal.strong } else { color },
            italics: style.italic,
            line_height: Some(size * 1.5),
            valign: Align::Center,
            ..Default::default()
        };
        if self.variable && !style.code {
            f.coords = VariationCoords::new([(b"wght", if bold { 680.0 } else { 400.0 })]);
        }
        if style.code {
            f.color = self.pal.code_fg;
            f.background = self.pal.code_bg;
            f.expand_bg = 2.0;
        }
        if style.strike {
            f.strikethrough = Stroke::new(1.0, f.color);
        }
        if style.html {
            f.color = self.pal.dim;
            f.font_id = FontId::monospace(size * 0.85);
        }
        if style.image {
            f.color = self.pal.dim;
            f.italics = true;
        }
        if style.note {
            f.font_id.size = size * 0.7;
            f.color = self.pal.link;
            f.valign = Align::Min;
        }
        f
    }

    fn job(&self, inline: &Inline, size: f32, color: Color32, bold: bool, width: f32) -> LayoutJob {
        let mut job = LayoutJob::default();
        job.wrap.max_width = width;
        for run in &inline.runs {
            let mut f = self.format(run.style, size, color, bold);
            if run.link.is_some() && !run.style.note {
                f.color = self.pal.link;
            }
            job.append(&inline.text[run.range.clone()], 0.0, f);
        }
        job
    }
}

/// The link under `pos` in a laid-out inline, if the pointer is over its text.
fn link_under<'a>(galley: &Galley, inline: &'a Inline, local: egui::Vec2) -> Option<&'a str> {
    let c = galley.cursor_from_pos(local);
    let char_rect = |i: usize| {
        let a = galley.pos_from_cursor(egui::text::CCursor::new(i));
        let b = galley.pos_from_cursor(egui::text::CCursor::new(i + 1));
        (a.min.y == b.min.y).then(|| Rect::from_x_y_ranges(a.min.x.min(b.min.x)..=a.min.x.max(b.min.x), a.min.y..=a.max.y))
    };
    for i in [c.index.0.saturating_sub(1), c.index.0] {
        if let Some(r) = char_rect(i)
            && r.expand(1.0).contains(local.to_pos2())
        {
            let byte = inline.text.char_indices().nth(i).map(|(b, _)| b)?;
            return inline.link_at(byte);
        }
    }
    None
}

/// Lay out and draw an inline; clicks on links land in `d.clicked`.
fn paragraph(ui: &mut Ui, d: &mut Draw, inline: &Inline, size: f32, color: Color32, bold: bool) -> Rect {
    let job = d.job(inline, size, color, bold, ui.available_width().max(20.0));
    let galley = ui.painter().layout_job(job);
    let (rect, resp) = ui.allocate_exact_size(galley.size(), if inline.links.is_empty() { Sense::hover() } else { Sense::click() });
    if !inline.links.is_empty()
        && let Some(p) = resp.hover_pos()
        && let Some(url) = link_under(&galley, inline, p - rect.min)
    {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        if resp.clicked() {
            d.clicked = Some(url.to_owned());
        }
        resp.on_hover_text_at_pointer(url.to_owned());
    }
    ui.painter().galley(rect.min, galley, color);
    rect
}

fn blocks(ui: &mut Ui, d: &mut Draw, blocks: &[Block], depth: usize, dim: bool, tight: bool) {
    for (i, b) in blocks.iter().enumerate() {
        if i > 0 {
            ui.add_space(if tight { 2.0 } else { d.size * 0.7 });
        }
        block(ui, d, b, depth, dim, i);
    }
}

fn block(ui: &mut Ui, d: &mut Draw, b: &Block, depth: usize, dim: bool, index: usize) {
    let pal = d.pal.clone();
    let text = if dim { pal.dim } else { pal.fg };
    match b {
        Block::Paragraph(inline) => {
            paragraph(ui, d, inline, d.size, text, false);
        }
        Block::Heading { level, text: inline, .. } => {
            let scale = [1.9, 1.5, 1.25, 1.1, 1.0, 0.92][(*level as usize).clamp(1, 6) - 1];
            let color = if *level >= 6 { pal.dim } else if *level <= 3 { pal.heading } else { pal.strong };
            if index > 0 {
                ui.add_space(d.size * 0.5);
            }
            let r = paragraph(ui, d, inline, d.size * scale, color, true);
            if *level <= 2 {
                ui.add_space(4.0);
                let y = ui.cursor().top();
                ui.painter().hline(r.left()..=ui.max_rect().right(), y, Stroke::new(1.0, pal.rule));
                ui.add_space(4.0);
            }
        }
        Block::Code { lang, code } => code_block(ui, d, lang, code),
        Block::Quote { kind, blocks: kids } => {
            let bar = match *kind {
                Some("Note") => Color32::from_rgb(0x44, 0x93, 0xf8),
                Some("Tip") => Color32::from_rgb(0x3f, 0xb9, 0x50),
                Some("Important") => Color32::from_rgb(0xab, 0x7d, 0xf8),
                Some("Warning") => Color32::from_rgb(0xd2, 0x99, 0x22),
                Some("Caution") => Color32::from_rgb(0xf8, 0x51, 0x49),
                _ => mix(pal.accent, pal.bg, 0.45),
            };
            let top = ui.cursor().top();
            let left = ui.max_rect().left();
            let inner = Rect::from_min_max(pos2(left + 16.0, top), pos2(ui.max_rect().right(), top + 1e6));
            let r = ui
                .scope_builder(UiBuilder::new().max_rect(inner).layout(Layout::top_down(Align::Min)), |ui| {
                    ui.add_space(2.0);
                    if let Some(label) = kind {
                        let mut f = d.format(Style::default(), d.size, bar, true);
                        f.color = bar;
                        let job = LayoutJob::single_section((*label).to_owned(), f);
                        ui.label(job);
                    }
                    blocks(ui, d, kids, depth, true, false);
                    ui.add_space(2.0);
                })
                .response
                .rect;
            ui.painter().rect_filled(Rect::from_x_y_ranges(left + 2.0..=left + 5.0, r.top()..=r.bottom()), 1.5, bar);
        }
        Block::List { start, items } => list(ui, d, *start, items, depth, dim),
        Block::Table { aligns, head, rows } => table(ui, d, aligns, head, rows),
        Block::Rule => {
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), d.size), Sense::hover());
            ui.painter().hline(r.x_range(), r.center().y, Stroke::new(1.5, pal.rule));
        }
        Block::Html(html) => {
            let f = TextFormat { font_id: FontId::monospace(d.size * 0.85), color: pal.dim, ..Default::default() };
            let mut job = LayoutJob::single_section(html.clone(), f);
            job.wrap.max_width = ui.available_width();
            ui.label(job);
        }
        Block::Footnote { label, blocks: kids } => {
            let top = ui.cursor().top();
            let left = ui.max_rect().left();
            let small = d.small;
            ui.painter().text(pos2(left, top + small * 0.75), egui::Align2::LEFT_CENTER, format!("{label}."), FontId::new(small, d.family.clone()), pal.link);
            let inner = Rect::from_min_max(pos2(left + 28.0, top), pos2(ui.max_rect().right(), top + 1e6));
            let saved = d.size;
            d.size = small;
            ui.scope_builder(UiBuilder::new().max_rect(inner).layout(Layout::top_down(Align::Min)), |ui| blocks(ui, d, kids, depth, true, true));
            d.size = saved;
        }
    }
}

fn code_block(ui: &mut Ui, d: &mut Draw, lang: &str, code: &str) {
    let pal = d.pal.clone();
    let size = d.size * 0.88;
    let key = {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (lang, code).hash(&mut h);
        h.finish()
    };
    if d.code.len() > 512 {
        d.code.clear();
    }
    let spans = d.code.entry(key).or_insert_with(|| highlight::highlight_snippet(code, lang, d.theme).map(Arc::new)).clone();
    egui::Frame::new()
        .fill(pal.code_bg)
        .stroke(Stroke::new(1.0, mix(pal.code_bg, pal.fg, 0.06)))
        .corner_radius(6.0)
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let font = FontId::monospace(size);
            let mut job = LayoutJob::default();
            job.wrap.max_width = ui.available_width();
            let fmt = |color: Color32, italics: bool| TextFormat { font_id: font.clone(), color, italics, line_height: Some(size * 1.45), ..Default::default() };
            match &spans {
                Some(spans) => {
                    let mut end = 0;
                    for s in spans.iter() {
                        if let Some(piece) = code.get(s.range.clone()) {
                            job.append(piece, 0.0, fmt(s.color, s.italic));
                            end = s.range.end;
                        }
                    }
                    if end < code.len() {
                        job.append(&code[end..], 0.0, fmt(pal.fg, false));
                    }
                }
                None => job.append(code, 0.0, fmt(pal.fg, false)),
            }
            let galley = ui.painter().layout_job(job);
            let (rect, _) = ui.allocate_exact_size(galley.size(), Sense::hover());
            ui.painter().galley(rect.min, galley, pal.fg);
            if !lang.is_empty() {
                let r = ui.max_rect();
                ui.painter().text(pos2(r.right() + 6.0, r.top() - 4.0), egui::Align2::RIGHT_TOP, lang, FontId::proportional(d.small * 0.9), pal.dim);
            }
        });
}

fn list(ui: &mut Ui, d: &mut Draw, start: Option<u64>, items: &[Item], depth: usize, dim: bool) {
    let pal = d.pal.clone();
    let tight = !items.iter().any(|it| it.loose);
    let indent = if start.is_some() { d.size * 1.9 } else { d.size * 1.4 };
    let line_h = d.size * 1.5;
    for (n, item) in items.iter().enumerate() {
        if n > 0 {
            ui.add_space(if tight { 3.0 } else { d.size * 0.5 });
        }
        let top = ui.cursor().top();
        let left = ui.max_rect().left();
        let mid = top + line_h / 2.0;
        let color = if dim { pal.dim } else { pal.fg };
        let text_left = left + indent + if item.task.is_some() { d.size * 1.4 } else { 0.0 };
        match start {
            Some(first) => {
                let label = format!("{}.", first + n as u64);
                ui.painter().text(pos2(left + indent - d.size * 0.45, mid), egui::Align2::RIGHT_CENTER, label, FontId::new(d.size, d.family.clone()), pal.dim);
            }
            None if item.task.is_none() => {
                let c = pos2(left + indent * 0.5, mid);
                let r = d.size * 0.17;
                match depth % 3 {
                    0 => ui.painter().circle_filled(c, r, pal.dim),
                    1 => ui.painter().circle_stroke(c, r, Stroke::new(1.2, pal.dim)),
                    _ => ui.painter().rect_filled(Rect::from_center_size(c, vec2(r * 1.8, r * 1.8)), 0.0, pal.dim),
                };
            }
            None => {}
        }
        if let Some(done) = item.task {
            let s = d.size * 0.95;
            let x = if start.is_some() { left + indent } else { left + indent * 0.25 };
            let bx = Rect::from_min_size(pos2(x, mid - s / 2.0), vec2(s, s));
            if done {
                ui.painter().rect_filled(bx, 3.0, pal.accent);
                let check = [pos2(bx.left() + s * 0.22, bx.center().y), pos2(bx.left() + s * 0.43, bx.bottom() - s * 0.26), pos2(bx.right() - s * 0.2, bx.top() + s * 0.25)];
                ui.painter().line(check.to_vec(), Stroke::new(1.8, pal.bg));
            } else {
                ui.painter().rect_stroke(bx, 3.0, Stroke::new(1.3, pal.dim), egui::StrokeKind::Inside);
            }
        }
        let text_left = if item.task.is_some() && start.is_none() { left + indent * 0.25 + d.size * 1.45 } else { text_left };
        let inner = Rect::from_min_max(pos2(text_left, top), pos2(ui.max_rect().right(), top + 1e6));
        ui.scope_builder(UiBuilder::new().max_rect(inner).layout(Layout::top_down(Align::Min)), |ui| {
            if item.blocks.is_empty() {
                ui.allocate_exact_size(vec2(1.0, line_h), Sense::hover());
            }
            blocks(ui, d, &item.blocks, depth + 1, color == pal.dim, tight);
        });
    }
}

fn table(ui: &mut Ui, d: &mut Draw, aligns: &[Align], head: &[Inline], rows: &[Vec<Inline>]) {
    let pal = d.pal.clone();
    let ncols = rows.iter().map(Vec::len).chain([head.len()]).max().unwrap_or(0);
    if ncols == 0 {
        return;
    }
    let pad = vec2(10.0, 5.0);
    let size = d.size * 0.95;
    let all: Vec<(&[Inline], bool)> = std::iter::once((head, true)).chain(rows.iter().map(|r| (r.as_slice(), false))).collect();
    let empty = Inline::default();
    fn cell<'a>(r: &'a [Inline], c: usize, empty: &'a Inline) -> &'a Inline {
        r.get(c).unwrap_or(empty)
    }
    let layout = |ui: &Ui, inl: &Inline, bold: bool, w: f32| ui.painter().layout_job(d.job(inl, size, pal.fg, bold, w));

    // Natural column widths, then shrink the widest columns to fit.
    let mut natural = vec![0.0f32; ncols];
    for (r, bold) in &all {
        for (c, w) in natural.iter_mut().enumerate() {
            *w = w.max(layout(ui, cell(r, c, &empty), *bold, f32::INFINITY).size().x.ceil());
        }
    }
    let avail = (ui.available_width() - 2.0 * pad.x * ncols as f32 - 1.0).max(40.0 * ncols as f32);
    let mut widths = natural.clone();
    if natural.iter().sum::<f32>() > avail {
        // Water-filling: narrow columns keep their width, wide ones share the rest.
        let mut sorted = natural.clone();
        sorted.sort_by(f32::total_cmp);
        let (mut left, mut cap) = (avail, avail / ncols as f32);
        for (k, w) in sorted.iter().enumerate() {
            cap = left / (ncols - k) as f32;
            if *w > cap {
                break;
            }
            left -= w;
        }
        for w in &mut widths {
            *w = w.min(cap.max(40.0));
        }
    }
    let galleys: Vec<Vec<Arc<Galley>>> = all.iter().map(|(r, bold)| (0..ncols).map(|c| layout(ui, cell(r, c, &empty), *bold, widths[c])).collect()).collect();
    let heights: Vec<f32> = galleys.iter().map(|row| row.iter().map(|g| g.size().y).fold(size * 1.5, f32::max) + 2.0 * pad.y).collect();
    let total = vec2(widths.iter().sum::<f32>() + 2.0 * pad.x * ncols as f32, heights.iter().sum());
    let (rect, resp) = ui.allocate_exact_size(total, Sense::click());
    let p = ui.painter();
    let mut y = rect.top();
    for (ri, (row, h)) in galleys.iter().zip(&heights).enumerate() {
        let row_rect = Rect::from_min_size(pos2(rect.left(), y), vec2(total.x, *h));
        if ri == 0 {
            p.rect_filled(row_rect, egui::CornerRadius { nw: 4, ne: 4, sw: 0, se: 0 }, pal.code_bg);
        } else if ri % 2 == 0 {
            p.rect_filled(row_rect, 0.0, mix(pal.bg, pal.fg, 0.03));
        }
        let mut x = rect.left();
        for (c, g) in row.iter().enumerate() {
            let cw = widths[c] + 2.0 * pad.x;
            let gx = match aligns.get(c).copied().unwrap_or(Align::Min) {
                Align::Min => x + pad.x,
                Align::Center => x + (cw - g.size().x) / 2.0,
                Align::Max => x + cw - pad.x - g.size().x,
            };
            let at = pos2(gx, y + pad.y);
            if let Some(hp) = resp.hover_pos()
                && Rect::from_min_size(at, g.size()).contains(hp)
            {
                let inl = cell(all[ri].0, c, &empty);
                if let Some(url) = link_under(g, inl, hp - at) {
                    ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                    if resp.clicked() {
                        d.clicked = Some(url.to_owned());
                    }
                }
            }
            p.galley(at, g.clone(), pal.fg);
            x += cw;
        }
        y += h;
    }
    // Grid lines: header rule stronger.
    let stroke = Stroke::new(1.0, pal.rule);
    let mut y = rect.top();
    for (ri, h) in heights.iter().enumerate() {
        y += h;
        if ri + 1 < heights.len() {
            let s = if ri == 0 { Stroke::new(1.5, mix(pal.rule, pal.fg, 0.25)) } else { stroke };
            p.hline(rect.x_range(), y, s);
        }
    }
    let mut x = rect.left();
    for w in &widths[..ncols - 1] {
        x += w + 2.0 * pad.x;
        p.vline(x, rect.y_range(), stroke);
    }
    p.rect_stroke(rect, 4.0, stroke, egui::StrokeKind::Inside);
}

// ---- the panel --------------------------------------------------------------------

/// egui font family for preview text: the system UI font when it's a
/// variable font (real bold weights), else egui's proportional font.
const FAMILY: &str = "strata-markdown";

fn body_font(ctx: &egui::Context, attempted: &mut u64) -> (FontFamily, bool) {
    let name = FontFamily::Name(FAMILY.into());
    let (has, custom_ui, face) = ctx.fonts(|f| {
        let defs = f.definitions();
        let face = ["macos-symbols", "strata-markdown-face"].into_iter().find(|k| defs.font_data.contains_key(*k));
        (defs.families.contains_key(&name), defs.font_data.contains_key("strata-ui"), face)
    });
    // A chosen interface font wins.
    if custom_ui {
        return (FontFamily::Proportional, false);
    }
    if has {
        return (name, true);
    }
    // The window sets its own fonts on the first pass; adding ours on top of
    // a set that is about to be replaced would undo the user's font choice.
    let pass = ctx.cumulative_pass_nr();
    if pass < 2 || (*attempted != 0 && pass < *attempted + 10) {
        return (FontFamily::Proportional, false);
    }
    *attempted = pass;
    let mut defs = ctx.fonts(|f| f.definitions().clone());
    let face = match face {
        Some(k) => k.to_owned(),
        None => {
            // Windows 11's variable Segoe UI.
            static SEGOE: std::sync::LazyLock<Option<Vec<u8>>> =
                std::sync::LazyLock::new(|| cfg!(windows).then(|| std::fs::read("C:\\Windows\\Fonts\\SegUIVar.ttf").ok()).flatten());
            let Some(bytes) = SEGOE.clone() else {
                return (FontFamily::Proportional, false);
            };
            defs.font_data.insert("strata-markdown-face".into(), Arc::new(egui::FontData::from_owned(bytes)));
            "strata-markdown-face".into()
        }
    };
    let mut fam = vec![face.clone()];
    fam.extend(defs.families.get(&FontFamily::Proportional).into_iter().flatten().filter(|n| **n != face).cloned());
    defs.families.insert(name, fam);
    ctx.set_fonts(defs);
    (FontFamily::Proportional, false)
}

/// The preview panel's state: parsed blocks for the shown document, measured
/// block heights, highlighted code blocks.
pub struct Preview {
    /// Base text size (follows the editor font size).
    pub font_size: f32,
    /// (view, edit version, length) of what `blocks` was parsed from.
    key: Option<(egui::Id, u64, usize)>,
    blocks: Arc<Vec<TopBlock>>,
    parsed_at: f64,
    /// Measured height per block source hash, for `heights_key`.
    heights: HashMap<u64, f32>,
    heights_key: (u32, u32),
    /// Each block's y in the scrolled content, from the last frame.
    tops: Vec<f32>,
    code: HashMap<u64, Option<Arc<Vec<Span>>>>,
    code_theme: String,
    /// Editor top line the preview last followed.
    synced: Option<(egui::Id, usize)>,
    scroll_to: Option<f32>,
    font_attempt: u64,
}

impl Default for Preview {
    fn default() -> Self {
        Self {
            font_size: 14.0,
            key: None,
            blocks: Arc::default(),
            parsed_at: f64::NEG_INFINITY,
            heights: HashMap::new(),
            heights_key: (0, 0),
            tops: Vec::new(),
            code: HashMap::new(),
            code_theme: String::new(),
            synced: None,
            scroll_to: None,
            font_attempt: 0,
        }
    }
}

const MAX_WIDTH: f32 = 860.0;
const MARGIN: f32 = 24.0;
/// Above this size, re-parse at most every `DEBOUNCE` seconds while typing.
const LARGE: usize = 256 * 1024;
const DEBOUNCE: f64 = 0.3;

impl Preview {
    #[cfg(test)]
    pub fn blocks(&self) -> &[TopBlock] {
        &self.blocks
    }

    /// Re-parse if `doc` changed since last time (rate-limited for big files).
    pub fn refresh(&mut self, doc: &mut Document, now: f64) -> bool {
        let key = (doc.view.id, doc.editor.version(), doc.editor.buffer.len_chars());
        if self.key == Some(key) {
            return true;
        }
        let same_doc = self.key.is_some_and(|k| k.0 == key.0);
        if same_doc && key.2 > LARGE && now - self.parsed_at < DEBOUNCE {
            return false;
        }
        if !same_doc {
            self.synced = None;
        }
        self.key = Some(key);
        self.parsed_at = now;
        let text = doc.editor.text_snapshot();
        self.blocks = Arc::new(parse(&text));
        true
    }

    fn estimate(&self, b: &TopBlock, width: f32) -> f32 {
        if let Some(h) = self.heights.get(&b.hash) {
            return *h;
        }
        let line = self.font_size * 1.5;
        let per_line = (width / (self.font_size * 0.5)).max(10.0);
        match &b.block {
            Block::Code { .. } | Block::Table { .. } => b.lines as f32 * line + 22.0,
            Block::Heading { level, .. } => line * if *level <= 2 { 1.9 } else { 1.3 },
            Block::Rule => self.font_size,
            _ => (b.chars as f32 / per_line).ceil().max(b.lines as f32) * line,
        }
    }

    /// Draw the preview of `doc` (or a hint when it isn't Markdown).
    pub fn show(&mut self, ui: &mut Ui, doc: &mut Document) -> Option<Action> {
        let ctx = ui.ctx().clone();
        let pal = Palette::new(&doc.highlighter);
        if !is_markdown(doc.path.as_deref(), doc.highlighter.syntax_name()) {
            let r = ui.max_rect();
            ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, "Markdown preview\nopen a .md file to see it here", FontId::proportional(13.0), pal.dim);
            ui.allocate_rect(r, Sense::hover());
            return None;
        }
        let now = ctx.input(|i| i.time);
        if !self.refresh(doc, now) {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(DEBOUNCE));
        }
        if self.code_theme != doc.highlighter.theme_key() {
            self.code.clear();
            self.code_theme = doc.highlighter.theme_key();
            self.heights.clear();
        }
        let (family, variable) = body_font(&ctx, &mut self.font_attempt);
        let view_id = doc.view.id;

        // Follow the editor: put the block holding its top line at the top.
        let top_line = doc.view.top_line();
        if self.synced != Some((view_id, top_line)) {
            self.synced = Some((view_id, top_line));
            if let Some(y) = self.y_for_line(top_line) {
                self.scroll_to = Some(y);
            }
        }

        let blocks = self.blocks.clone();
        let mut area = egui::ScrollArea::vertical().id_salt(("markdown-preview", view_id)).auto_shrink(false);
        if let Some(y) = self.scroll_to.take() {
            area = area.vertical_scroll_offset(y.max(0.0));
        }
        let mut clicked = None;
        area.show_viewport(ui, |ui, viewport| {
            let full = ui.available_width();
            let width = (full - 2.0 * MARGIN).clamp(60.0, MAX_WIDTH);
            let x0 = ((full - width) / 2.0).max(8.0);
            let hk = (width.round() as u32, self.font_size.to_bits());
            if hk != self.heights_key {
                self.heights_key = hk;
                self.heights.clear();
            }
            if blocks.is_empty() {
                let r = ui.max_rect();
                ui.painter().text(r.center_top() + vec2(0.0, 60.0), egui::Align2::CENTER_CENTER, "Nothing to preview yet", FontId::proportional(13.0), pal.dim);
                return;
            }
            let gap = self.font_size * 0.9;
            self.tops.clear();
            let mut y = MARGIN * 0.75;
            for b in blocks.iter() {
                self.tops.push(y);
                y += self.estimate(b, width) + gap;
            }
            let total = y + MARGIN * 2.0;
            let origin = ui.max_rect().min;
            let first = self.tops.partition_point(|&t| t <= viewport.min.y).saturating_sub(1);
            let mut d = Draw {
                pal: &pal,
                family,
                variable,
                size: self.font_size,
                theme: doc.highlighter.theme_name(),
                code: &mut self.code,
                clicked: None,
                small: self.font_size * 0.9,
            };
            let mut changed = false;
            for i in first..blocks.len() {
                let top = self.tops[i];
                if top > viewport.max.y {
                    break;
                }
                let b = &blocks[i];
                let rect = Rect::from_min_size(origin + vec2(x0, top), vec2(width, 1e6));
                let mut child = ui.new_child(UiBuilder::new().max_rect(rect).layout(Layout::top_down(Align::Min)).id_salt(("md-block", i)));
                block(&mut child, &mut d, &b.block, 0, false, i);
                let h = child.min_rect().height();
                let known = self.heights.get(&b.hash).copied();
                if known.is_none_or(|k| (k - h).abs() > 0.5) {
                    self.heights.insert(b.hash, h);
                    changed = true;
                }
            }
            clicked = d.clicked.take();
            ui.set_min_size(vec2(full, total));
            if changed {
                ui.ctx().request_repaint();
            }
        });
        let url = clicked?;
        self.follow(&ctx, &url, doc.path.as_deref())
    }

    /// Content y for buffer `line`, interpolated within its block.
    fn y_for_line(&self, line: usize) -> Option<f32> {
        if self.tops.len() != self.blocks.len() || self.blocks.is_empty() {
            return None;
        }
        let i = self.blocks.partition_point(|b| b.line <= line).checked_sub(1)?;
        let (l0, y0) = (self.blocks[i].line, self.tops[i]);
        let (l1, y1) = match self.blocks.get(i + 1) {
            Some(b) => (b.line, self.tops[i + 1]),
            None => (l0 + self.blocks[i].lines, y0 + self.heights.get(&self.blocks[i].hash).copied().unwrap_or(0.0)),
        };
        let t = if l1 > l0 { ((line - l0) as f32 / (l1 - l0) as f32).min(1.0) } else { 0.0 };
        Some(if line == 0 { 0.0 } else { y0 + (y1 - y0) * t - MARGIN * 0.5 })
    }

    fn follow(&mut self, ctx: &egui::Context, url: &str, base: Option<&Path>) -> Option<Action> {
        if let Some(anchor) = url.strip_prefix('#') {
            let i = self.blocks.iter().position(|b| matches!(&b.block, Block::Heading { slug, .. } if slug == anchor))?;
            self.scroll_to = self.tops.get(i).map(|y| y - MARGIN * 0.5);
            ctx.request_repaint();
            return None;
        }
        let scheme = url.split_once(':').is_some_and(|(s, _)| s.len() > 1 && s.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)));
        if scheme {
            ctx.open_url(egui::OpenUrl::new_tab(url));
            return None;
        }
        let path = url.split('#').next().unwrap_or(url).replace("%20", " ");
        let dir = base.and_then(Path::parent).map(Path::to_path_buf).unwrap_or_default();
        let target = dir.join(path);
        if !target.exists() {
            return Some(Action::Status(format!("Link target not found: {}", target.display())));
        }
        let text_like = target.is_file() && std::fs::read(&target).map(|b| !b.iter().take(8000).any(|&c| c == 0)).unwrap_or(false);
        if text_like {
            Some(Action::Open(target))
        } else {
            ctx.open_url(egui::OpenUrl::new_tab(format!("file://{}", target.display())));
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# Title\n\nSome *em* and **strong** and ~~gone~~ and `code` and [a link](https://x.dev).\n\n## Lists\n\n- one\n- two\n  - nested\n- [ ] todo\n- [x] done\n\n1. first\n2. second\n\n> quoted **text**\n\n```rust\nfn main() {}\n```\n\n| A | B |\n|:--|--:|\n| 1 | 2 |\n| 3 | 4 |\n\n---\n\n![alt text](img.png)\n\nNote[^1].\n\n[^1]: The note.\n";

    fn top(src: &str) -> Vec<Block> {
        parse(src).into_iter().map(|b| b.block).collect()
    }

    #[test]
    fn headings_paragraphs_and_inline_styles() {
        let blocks = top(SAMPLE);
        assert!(matches!(&blocks[0], Block::Heading { level: 1, text, slug } if text.text == "Title" && slug == "title"));
        let Block::Paragraph(p) = &blocks[1] else { panic!("{:?}", blocks[1]) };
        let style_of = |word: &str| {
            let at = p.text.find(word).unwrap();
            p.runs.iter().find(|r| r.range.contains(&at)).unwrap().clone()
        };
        assert!(style_of("em").style.italic);
        assert!(style_of("strong").style.bold);
        assert!(style_of("gone").style.strike);
        assert!(style_of("code").style.code);
        assert_eq!(p.link_at(p.text.find("a link").unwrap()), Some("https://x.dev"));
        assert_eq!(slug("Hello, World! v2_x"), "hello-world-v2_x");
    }

    #[test]
    fn lists_tasks_and_nesting() {
        let blocks = top(SAMPLE);
        let Block::List { start: None, items } = &blocks[3] else { panic!("{:?}", blocks[3]) };
        assert_eq!(items.len(), 4);
        assert!(matches!(&items[1].blocks[1], Block::List { items, .. } if items.len() == 1));
        assert_eq!(items[2].task, Some(false));
        assert_eq!(items[3].task, Some(true));
        assert_eq!(items[0].task, None);
        assert!(matches!(&blocks[4], Block::List { start: Some(1), items } if items.len() == 2));
        assert!(matches!(&blocks[5], Block::Quote { kind: None, blocks } if matches!(&blocks[0], Block::Paragraph(p) if p.text == "quoted text")));
    }

    #[test]
    fn code_tables_rules_images_footnotes() {
        let blocks = top(SAMPLE);
        assert!(matches!(&blocks[6], Block::Code { lang, code } if lang == "rust" && code == "fn main() {}"));
        let Block::Table { aligns, head, rows } = &blocks[7] else { panic!("{:?}", blocks[7]) };
        assert_eq!(aligns, &[Align::Min, Align::Max]);
        assert_eq!(head.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(), ["A", "B"]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1][1].text, "4");
        assert_eq!(blocks[8], Block::Rule);
        assert!(matches!(&blocks[9], Block::Paragraph(p) if p.text == "🖼 alt text" && p.runs[0].style.image));
        assert!(matches!(&blocks[10], Block::Paragraph(p) if p.runs.iter().any(|r| r.style.note)));
        assert!(matches!(&blocks[11], Block::Footnote { label, .. } if label == "1"));
        assert!(highlight::highlight_snippet("fn main() {}", "rust", highlight::DEFAULT_THEME).is_some());
        assert!(highlight::highlight_snippet("x", "no-such-lang", highlight::DEFAULT_THEME).is_none());
    }

    #[test]
    fn blocks_know_their_source_lines() {
        let b = parse("# A\n\npara\n\n\n## B\n");
        assert_eq!(b.iter().map(|b| b.line).collect::<Vec<_>>(), [0, 2, 5]);
        assert!(matches!(&top("> [!WARNING]\n> careful\n")[0], Block::Quote { kind: Some("Warning"), .. }));
        assert!(top("<!-- hidden -->\n").is_empty());
        let loose = top("1. a\n\n   b\n\n2. c\n");
        assert!(matches!(&loose[0], Block::List { items, .. } if items[0].loose && items[0].blocks.len() == 2));
        assert!(matches!(&top(SAMPLE)[3], Block::List { items, .. } if !items[0].loose));
    }

    #[test]
    fn html_export_is_a_standalone_page() {
        let html = to_html_document(SAMPLE, "fallback");
        assert!(html.starts_with("<!DOCTYPE html>"));
        assert!(html.contains("<title>Title</title>"));
        assert!(html.contains("<table>") && html.contains("<del>gone</del>"));
        assert!(html.contains("type=\"checkbox\""));
        assert!(html.contains("class=\"language-rust\""));
        assert!(to_html_document("plain <b>", "a & b").contains("<title>a &amp; b</title>"));
    }

    #[test]
    fn a_megabyte_parses_quickly() {
        let doc = SAMPLE.repeat(1_000_000 / SAMPLE.len() + 1);
        let t = std::time::Instant::now();
        let blocks = parse(&doc);
        assert!(blocks.len() > 10_000);
        // Generous: debug builds are slow; this guards against accidental quadratic work.
        assert!(t.elapsed().as_secs_f32() < 5.0, "{:?}", t.elapsed());
    }

    #[test]
    fn preview_renders_headless_and_follows_links() {
        let dir = std::env::temp_dir().join(format!("strata-md-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("other.md"), "# Other\n").unwrap();
        let path = dir.join("a.md");
        std::fs::write(&path, SAMPLE).unwrap();
        let mut doc = Document::from_buffer(Some(path.clone()), crate::buffer::Buffer::from_str(SAMPLE));
        doc.highlighter.detect(Some(&path), "");
        let mut preview = Preview::default();
        let ctx = egui::Context::default();
        for _ in 0..3 {
            let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(700.0, 900.0))), ..Default::default() };
            let mut out = ctx.run_ui(raw, |ui| {
                preview.show(ui, &mut doc);
            });
            out.textures_delta.clear();
        }
        assert_eq!(preview.blocks().len(), 12);
        assert_eq!(preview.tops.len(), 12);
        assert!(preview.tops.windows(2).all(|w| w[0] < w[1]));
        assert!(preview.heights.len() >= 10, "visible blocks were measured");
        // Scroll sync: line 0 is the top; a block's first line maps near its top.
        assert_eq!(preview.y_for_line(0), Some(0.0));
        let b = &preview.blocks()[6];
        assert_eq!(preview.y_for_line(b.line), Some(preview.tops[6] - MARGIN * 0.5));
        assert!(preview.y_for_line(b.line + 1).unwrap() > preview.tops[6] - MARGIN * 0.5);
        assert_eq!(preview.follow(&ctx, "other.md#x", Some(&path)), Some(Action::Open(dir.join("other.md"))));
        assert!(matches!(preview.follow(&ctx, "missing.md", Some(&path)), Some(Action::Status(_))));
        assert_eq!(preview.follow(&ctx, "#lists", Some(&path)), None);
        assert!(preview.scroll_to.is_some());
        // Editing re-parses.
        doc.editor.insert_text("Intro\n\n");
        assert!(preview.refresh(&mut doc, 10.0));
        assert_eq!(preview.blocks().len(), 13);
        std::fs::remove_dir_all(dir).ok();
    }
}
