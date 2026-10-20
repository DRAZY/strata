//! Markdown live preview: pulldown-cmark parses the active document into a
//! small block model, which is drawn with egui in the current color scheme.
//!
//! Parsing happens once per edit (debounced for very large files) and only
//! the blocks inside the viewport are laid out, using measured heights for
//! the rest, so a megabyte of Markdown stays responsive. Also: export as a
//! standalone HTML page.

use crate::document::Document;
use crate::highlight::{self, Highlighter, Span};
use crate::links::Target;
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

/// Notes in a vault also have `[[wikilinks]]` and YAML frontmatter.
fn note_options() -> Options {
    options() | Options::ENABLE_WIKILINKS | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS
}

/// The parser options the preview uses, as a note (`notes`) or not, so
/// other readers of the source (running code blocks) see the same blocks.
pub(crate) fn parser_options(notes: bool) -> Options {
    if notes { note_options() } else { options() }
}

/// Link targets that aren't URLs: a wikilink (`[[target]]`) and a tag. The
/// control character can't come from a Markdown link.
pub const NOTE_LINK: &str = "\u{1}note:";
pub const TAG_LINK: &str = "\u{1}tag:";

/// What `![[target]]` shows (see `Notes::embed`).
pub enum Embed {
    Image(egui::load::SizedTexture),
    /// A note (or one heading's section of it), already parsed.
    Note { title: String, blocks: Arc<Vec<TopBlock>> },
    /// Still being read; ask again next frame.
    Loading,
    /// Shown as a link: the target is missing, too deep, or embeds itself.
    Link(String),
}

/// What the preview of a note asks the notes index (see `app/notes_ui.rs`).
pub trait Notes {
    /// Does wikilink `target` (as written, `#subpath` included) find a file?
    fn resolves(&self, target: &str) -> bool;
    /// What to show for `![[target]]` (`wiki`) or `![](target)`, `depth` embeds deep.
    fn embed(&self, ctx: &egui::Context, target: &str, wiki: bool, depth: usize) -> Embed;
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
    /// A note's `#tag`.
    pub tag: bool,
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
    /// `run`: where a closed fenced block starts in the source (its opening
    /// fence), which is how the preview's Run button names it.
    Code { lang: String, code: String, run: Option<usize> },
    /// `kind` is a GitHub alert label ("Note", "Warning", …).
    Quote { kind: Option<&'static str>, blocks: Vec<Block> },
    List { start: Option<u64>, items: Vec<Item> },
    Table { aligns: Vec<Align>, head: Vec<Inline>, rows: Vec<Vec<Inline>> },
    Rule,
    Html(String),
    Footnote { label: String, blocks: Vec<Block> },
    /// A paragraph that is only `![[target]]` (`wiki`) or `![](target)`, in a note.
    Embed { target: String, wiki: bool },
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
    parse_as(src, false)
}

/// Parse a note: wikilinks, embeds, tags and frontmatter too.
pub fn parse_note(src: &str) -> Vec<TopBlock> {
    parse_as(src, true)
}

fn parse_as(src: &str, notes: bool) -> Vec<TopBlock> {
    let opts = if notes { note_options() } else { options() };
    let mut b = Builder { src, it: pulldown_cmark::Parser::new_ext(src, opts).into_offset_iter(), tasks: Vec::new(), paragraphs: false, notes };
    let mut out = Vec::new();
    let (mut line, mut counted) = (0, 0);
    while let Some((ev, range)) = b.it.next() {
        let block = match ev {
            Event::Start(tag) => b.block(tag, range.clone()),
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
    src: &'a str,
    it: pulldown_cmark::OffsetIter<'a, pulldown_cmark::DefaultBrokenLinkCallback>,
    /// Task marker of each open list item, innermost last.
    tasks: Vec<Option<bool>>,
    /// The last `blocks` call saw explicit paragraphs.
    paragraphs: bool,
    /// A note: wikilinks, embeds, tags.
    notes: bool,
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
    /// A note: tags are styled, wikilinks get `NOTE_LINK` targets.
    notes: bool,
    /// Images seen (target, wikilink?), and whether there was anything else,
    /// to tell a paragraph that is only an embed.
    images: Vec<(String, bool)>,
    other: bool,
    /// Text not yet added: the parser may split a run (at `_`, say), and a
    /// note's tags are found in whole runs.
    pending: String,
}

impl InlineState {
    fn new(notes: bool) -> Self {
        InlineState { notes, ..Default::default() }
    }

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
        if self.image.is_empty() && !s.trim().is_empty() {
            self.other = true;
        }
        self.inline.push(s, style, link);
    }

    /// Plain text, with a note's `#tags` (Obsidian's rules) styled and clickable.
    fn prose(&mut self, s: &str) {
        if !self.notes || !s.contains('#') || self.link().is_some() || !self.image.is_empty() {
            return self.text(s, |_| {});
        }
        let mut at = 0;
        let mut prev = self.inline.text.chars().last();
        let mut chars = s.char_indices().peekable();
        while let Some((i, c)) = chars.next() {
            let starts = c == '#' && prev.is_none_or(char::is_whitespace);
            prev = Some(c);
            if !starts {
                continue;
            }
            let body = &s[i + 1..];
            let len: usize = body.chars().take_while(|&c| crate::notes::parse::tag_char(c)).map(char::len_utf8).sum();
            let name = body[..len].trim_end_matches('/');
            if !crate::notes::parse::valid_tag(name) {
                continue;
            }
            self.text(&s[at..i], |_| {});
            self.inline.links.push(format!("{TAG_LINK}{name}"));
            let link = Some(self.inline.links.len() - 1);
            let style = Style { tag: true, ..self.style() };
            self.other = true;
            self.inline.push(&s[i..i + 1 + name.len()], style, link);
            at = i + 1 + name.len();
            while chars.peek().is_some_and(|&(k, _)| k < at) {
                chars.next();
            }
            prev = s[..at].chars().last();
        }
        self.text(&s[at..], |_| {});
    }

    /// Add text held back by `event`.
    fn flush(&mut self) {
        if !self.pending.is_empty() {
            let text = std::mem::take(&mut self.pending);
            self.prose(&text);
        }
    }

    /// The inline content so far, every piece of text added.
    fn finish(mut self) -> Self {
        self.flush();
        self
    }

    /// Feed one inline event; false if it isn't one.
    fn event(&mut self, ev: Event) -> bool {
        if let Event::Text(t) = &ev {
            self.pending.push_str(t);
            return true;
        }
        self.flush();
        match ev {
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
            Event::Start(Tag::Link { dest_url, link_type, .. }) => {
                let url = if matches!(link_type, pulldown_cmark::LinkType::WikiLink { .. }) { format!("{NOTE_LINK}{dest_url}") } else { dest_url.to_string() };
                self.inline.links.push(url);
                self.links.push(Some(self.inline.links.len() - 1));
            }
            Event::Start(Tag::Image { dest_url, link_type, .. }) => {
                self.images.push((dest_url.to_string(), matches!(link_type, pulldown_cmark::LinkType::WikiLink { .. })));
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
        self.inline_state(end).inline
    }

    fn inline_state(&mut self, end: TagEnd) -> InlineState {
        let mut st = InlineState::new(self.notes);
        while let Some(ev) = self.next() {
            match ev {
                Event::End(e) if e == end => break,
                Event::TaskListMarker(done) => self.set_task(done),
                ev => {
                    st.event(ev);
                }
            }
        }
        st.finish()
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
        let notes = self.notes;
        let mut loose = InlineState::new(notes);
        let mut paragraphs = false;
        let flush = |loose: &mut InlineState, out: &mut Vec<Block>| {
            let st = std::mem::replace(loose, InlineState::new(notes)).finish();
            if !st.inline.text.is_empty() {
                out.push(paragraph_block(st));
            }
        };
        while let Some((ev, range)) = self.it.next() {
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
                    out.extend(self.block(tag, range));
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

    fn block(&mut self, tag: Tag, range: std::ops::Range<usize>) -> Option<Block> {
        Some(match tag {
            Tag::Paragraph => paragraph_block(self.inline_state(TagEnd::Paragraph)),
            // A note's frontmatter: its properties, as written.
            Tag::MetadataBlock(_) => {
                let mut code = String::new();
                while let Some(ev) = self.next() {
                    match ev {
                        Event::Text(t) => code.push_str(&t),
                        Event::End(_) => break,
                        _ => {}
                    }
                }
                Block::Code { lang: "yaml".into(), code: code.trim_end().to_owned(), run: None }
            }
            Tag::Heading { level, .. } => {
                let text = self.inline(TagEnd::Heading(level));
                let slug = slug(&text.text);
                Block::Heading { level: level_num(level), text, slug }
            }
            Tag::BlockQuote(kind) => Block::Quote { kind: kind.map(alert_label), blocks: self.blocks() },
            Tag::CodeBlock(kind) => {
                let fenced = matches!(kind, CodeBlockKind::Fenced(_));
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
                let run = (fenced && crate::runblock::closed(self.src.get(range.clone()).unwrap_or(""), &code)).then_some(range.start);
                if code.ends_with('\n') {
                    code.pop();
                }
                Block::Code { lang, code, run }
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

/// A paragraph, or an embed when it's a note's paragraph holding one image alone.
fn paragraph_block(st: InlineState) -> Block {
    match &st.images[..] {
        [(target, wiki)] if st.notes && !st.other => Block::Embed { target: target.clone(), wiki: *wiki },
        _ => Block::Paragraph(st.inline),
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
:root{color-scheme:light dark;--fg:#1f2328;--bg:#fff;--dim:#59636e;--rule:#d1d9e0;--code:#f6f8fa;--link:#0969da;--note:#0969da;--tip:#1a7f37;--important:#8250df;--warning:#9a6700;--caution:#d1242f}
@media (prefers-color-scheme:dark){:root{--fg:#e6edf3;--bg:#0d1117;--dim:#9198a1;--rule:#3d444d;--code:#151b23;--link:#4493f8;--note:#4493f8;--tip:#3fb950;--important:#ab7df8;--warning:#d29922;--caution:#f85149}}
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
blockquote[class^=markdown-alert-]{color:var(--fg)}
blockquote[class^=markdown-alert-]::before{display:block;margin:0 0 .4em;padding-top:.5em;font-weight:600}
.markdown-alert-note{border-left-color:var(--note)}.markdown-alert-note::before{content:\"Note\";color:var(--note)}
.markdown-alert-tip{border-left-color:var(--tip)}.markdown-alert-tip::before{content:\"Tip\";color:var(--tip)}
.markdown-alert-important{border-left-color:var(--important)}.markdown-alert-important::before{content:\"Important\";color:var(--important)}
.markdown-alert-warning{border-left-color:var(--warning)}.markdown-alert-warning::before{content:\"Warning\";color:var(--warning)}
.markdown-alert-caution{border-left-color:var(--caution)}.markdown-alert-caution::before{content:\"Caution\";color:var(--caution)}";

/// Markdown `src` as HTML body markup (no page around it). Raw HTML in the
/// Markdown and link targets go through `crate::safe_html`, so an exported
/// page never carries a script.
pub fn to_html_fragment(src: &str) -> String {
    use crate::safe_html::{safe_url, sanitize};
    fn neutral(url: pulldown_cmark::CowStr<'_>) -> pulldown_cmark::CowStr<'_> {
        if safe_url(&url) { url } else { "#".into() }
    }
    let events = pulldown_cmark::Parser::new_ext(src, options()).map(|e| match e {
        Event::Html(h) => Event::Html(sanitize(&h).into()),
        Event::InlineHtml(h) => Event::InlineHtml(sanitize(&h).into()),
        Event::Start(Tag::Link { link_type, dest_url, title, id }) => Event::Start(Tag::Link { link_type, dest_url: neutral(dest_url), title, id }),
        Event::Start(Tag::Image { link_type, dest_url, title, id }) => Event::Start(Tag::Image { link_type, dest_url: neutral(dest_url), title, id }),
        e => e,
    });
    let mut body = String::new();
    pulldown_cmark::html::push_html(&mut body, events);
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
    /// A link to some other local file: show it in the file manager.
    Reveal(PathBuf),
    Status(String),
    /// A note's link (`markdown`: a Markdown link, still percent-encoded): the notes index resolves it.
    Note { link: String, markdown: bool },
    /// A note's `#tag` was clicked.
    Tag(String),
    /// A code block's Run button was clicked: the block starting at byte
    /// `at` of the source, showing `code` (to check the preview wasn't stale).
    RunCode { at: usize, code: String },
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
            // Quotes, list markers, labels: softer than the text, still AA on the page and in code blocks.
            dim: [bg, mix(bg, fg, 0.07)].iter().fold(mix(fg, bg, 0.35), |c, &on| highlight::themes::ensure_contrast(c, on, 4.6)),
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
    /// The notes index, for a note in a vault.
    notes: Option<&'a dyn Notes>,
    /// How many embeds deep this is drawn.
    depth: usize,
    /// Code blocks get a Run button (the live preview of a document; never
    /// help pages, AI answers or embedded notes).
    run: bool,
    /// The Run button clicked: (block start, its code).
    run_clicked: Option<(usize, String)>,
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
            if run.style.tag {
                f.color = self.pal.accent;
                f.background = self.pal.accent.gamma_multiply(0.14);
                f.expand_bg = 1.5;
            } else if let Some(url) = run.link.map(|i| inline.links[i].as_str()).filter(|_| !run.style.note) {
                // A wikilink to a note that doesn't exist yet is fainter, as in Obsidian.
                let missing = url.strip_prefix(NOTE_LINK).is_some_and(|t| self.notes.is_some_and(|n| !n.resolves(t)));
                f.color = if missing { mix(self.pal.link, self.pal.bg, 0.35) } else { self.pal.link };
            }
            // Hidden characters show as markers (see `hidden::append`), never raw.
            crate::hidden::append(&mut job, &inline.text[run.range.clone()], 0.0, f);
        }
        job
    }
}

/// What a link's tooltip says: the target, without the markers of wikilinks and tags.
fn link_tip(url: &str) -> String {
    let tip = match (url.strip_prefix(NOTE_LINK), url.strip_prefix(TAG_LINK)) {
        (Some(t), _) => t.to_owned(),
        (_, Some(t)) => format!("#{t}"),
        _ => url.to_owned(),
    };
    crate::hidden::plain(&tip).into_owned()
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
            // Markers are longer than the char they stand for.
            let byte = inline.text.char_indices().nth(crate::hidden::source_index(&inline.text, i)).map(|(b, _)| b)?;
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
        resp.on_hover_text_at_pointer(link_tip(url));
    }
    ui.painter().galley(rect.min, galley, color);
    rect
}

/// `![[target]]` in a note: the image, or the note (one level deep), in place.
fn embed_block(ui: &mut Ui, d: &mut Draw, target: &str, wiki: bool) {
    let pal = d.pal.clone();
    let embed = match d.notes {
        Some(n) => n.embed(ui.ctx(), target, wiki, d.depth),
        None => Embed::Link(target.to_owned()),
    };
    let link_line = |ui: &mut Ui, d: &mut Draw, label: String| {
        let mut inline = Inline::default();
        inline.links.push(if wiki { format!("{NOTE_LINK}{target}") } else { target.to_owned() });
        inline.push(&label, Style::default(), Some(0));
        paragraph(ui, d, &inline, d.size, pal.dim, false);
    };
    match embed {
        Embed::Image(tex) => {
            let w = ui.available_width();
            let scale = (w / tex.size.x).min(1.0);
            let resp = ui.add(egui::Image::from_texture(tex).fit_to_exact_size(tex.size * scale).sense(Sense::click()));
            resp.on_hover_text_at_pointer(crate::hidden::plain(target).into_owned());
        }
        Embed::Note { title, blocks: inner } => {
            let top = ui.cursor().top();
            let left = ui.max_rect().left();
            let rect = Rect::from_min_max(pos2(left + 14.0, top), pos2(ui.max_rect().right(), top + 1e6));
            let r = ui
                .scope_builder(UiBuilder::new().max_rect(rect).layout(Layout::top_down(Align::Min)), |ui| {
                    ui.add_space(4.0);
                    link_line(ui, d, format!("↪ {title}"));
                    ui.add_space(4.0);
                    d.depth += 1;
                    for (i, b) in inner.iter().enumerate() {
                        if i > 0 {
                            ui.add_space(d.size * 0.6);
                        }
                        block(ui, d, &b.block, 0, false, i);
                    }
                    d.depth -= 1;
                    ui.add_space(4.0);
                })
                .response
                .rect;
            ui.painter().rect_filled(Rect::from_x_y_ranges(left + 2.0..=left + 5.0, r.top()..=r.bottom()), 1.5, mix(pal.link, pal.bg, 0.4));
        }
        Embed::Loading => {
            ui.label(egui::RichText::new(format!("Loading {}…", crate::hidden::plain(target))).color(pal.dim).italics());
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        }
        Embed::Link(label) => link_line(ui, d, format!("↪ {label}")),
    }
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
        Block::Code { lang, code, run } => code_block(ui, d, lang, code, *run),
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
            let mut job = crate::hidden::job(html, f);
            job.wrap.max_width = ui.available_width();
            ui.label(job);
        }
        Block::Embed { target, wiki } => embed_block(ui, d, target, *wiki),
        Block::Footnote { label, blocks: kids } => {
            let top = ui.cursor().top();
            let left = ui.max_rect().left();
            let small = d.small;
            ui.painter().text(pos2(left, top + small * 0.75), egui::Align2::LEFT_CENTER, format!("{}.", crate::hidden::plain(label)), FontId::new(small, d.family.clone()), pal.link);
            let inner = Rect::from_min_max(pos2(left + 28.0, top), pos2(ui.max_rect().right(), top + 1e6));
            let saved = d.size;
            d.size = small;
            ui.scope_builder(UiBuilder::new().max_rect(inner).layout(Layout::top_down(Align::Min)), |ui| blocks(ui, d, kids, depth, true, true));
            d.size = saved;
        }
    }
}

fn code_block(ui: &mut Ui, d: &mut Draw, lang: &str, code: &str, run: Option<usize>) {
    let pal = d.pal.clone();
    // Only on a click: nothing runs when the preview draws.
    let runnable = run.filter(|_| d.run && d.depth == 0 && crate::runblock::lang(lang).is_some());
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
                            crate::hidden::append(&mut job, piece, 0.0, fmt(s.color, s.italic));
                            end = s.range.end;
                        }
                    }
                    if end < code.len() {
                        crate::hidden::append(&mut job, &code[end..], 0.0, fmt(pal.fg, false));
                    }
                }
                None => crate::hidden::append(&mut job, code, 0.0, fmt(pal.fg, false)),
            }
            let galley = ui.painter().layout_job(job);
            if let Some(at) = runnable {
                // A row of its own above the code: the language, and Run at the right.
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(crate::hidden::plain(lang)).size(d.small * 0.9).color(pal.dim));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let b = egui::Button::new(egui::RichText::new("▶ Run").size(d.small * 0.9)).small();
                        if ui.add(b).on_hover_text("Run this block in the terminal (Strata shows it and asks first)").clicked() {
                            d.run_clicked = Some((at, code.to_owned()));
                        }
                    });
                });
                ui.add_space(2.0);
                let (rect, _) = ui.allocate_exact_size(galley.size(), Sense::hover());
                ui.painter().galley(rect.min, galley, pal.fg);
                return;
            }
            // The language sits in the top-right corner. When the first line
            // reaches under it, the label gets a row of its own above the code.
            let label = (!lang.is_empty()).then(|| ui.painter().layout_no_wrap(crate::hidden::plain(lang).into_owned(), FontId::proportional(d.small * 0.9), pal.dim));
            let r = ui.max_rect();
            if let Some(label) = &label {
                let room = ui.available_width() + 6.0 - label.size().x - 8.0;
                if galley.rows.first().is_some_and(|row| row.rect().right() > room) {
                    ui.add_space(label.size().y - 4.0);
                }
            }
            let (rect, _) = ui.allocate_exact_size(galley.size(), Sense::hover());
            ui.painter().galley(rect.min, galley, pal.fg);
            if let Some(label) = label {
                let at = pos2(r.right() + 6.0 - label.size().x, r.top() - 4.0);
                ui.painter().galley(at, label, pal.dim);
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
    let mut hover_url = None;
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
                    hover_url = Some(url.to_owned());
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
    // The real destination, whatever the link text says.
    if let Some(url) = hover_url {
        resp.on_hover_text_at_pointer(link_tip(&url));
    }
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
    /// (view, edit version, length, as a note) of what `blocks` was parsed from.
    key: Option<(egui::Id, u64, usize, bool)>,
    /// The document shown is a note in a vault (set by `show_with`).
    notes_mode: bool,
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
            notes_mode: false,
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
        let key = (doc.view.id, doc.editor.version(), doc.editor.buffer.len_chars(), self.notes_mode);
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
        self.blocks = Arc::new(parse_as(&text, self.notes_mode));
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
            Block::Embed { .. } => line * 4.0,
            _ => (b.chars as f32 / per_line).ceil().max(b.lines as f32) * line,
        }
    }

    /// Draw the preview of `doc` (or a hint when it isn't Markdown).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn show(&mut self, ui: &mut Ui, doc: &mut Document) -> Option<Action> {
        self.show_with(ui, doc, None)
    }

    /// Draw the preview of `doc`; `notes` is the vault's index when it's a note.
    pub fn show_with(&mut self, ui: &mut Ui, doc: &mut Document, notes: Option<&dyn Notes>) -> Option<Action> {
        self.notes_mode = notes.is_some();
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
        let mut run_clicked = None;
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
                notes,
                depth: 0,
                run: true,
                run_clicked: None,
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
            run_clicked = d.run_clicked.take();
            ui.set_min_size(vec2(full, total));
            if changed {
                ui.ctx().request_repaint();
            }
        });
        if let Some((at, code)) = run_clicked {
            return Some(Action::RunCode { at, code });
        }
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
        if let Some(target) = url.strip_prefix(NOTE_LINK) {
            return Some(Action::Note { link: target.to_owned(), markdown: false });
        }
        if let Some(tag) = url.strip_prefix(TAG_LINK) {
            return Some(Action::Tag(tag.to_owned()));
        }
        match crate::links::classify(url, base.and_then(Path::parent)) {
            Target::Anchor(anchor) => {
                // Obsidian's `[x](#Heading%20Text)` names the heading's text, not a slug.
                let key = crate::notes::parse::heading_key(&crate::notes::parse::percent_decode(&anchor));
                let i = self.blocks.iter().position(|b| matches!(&b.block, Block::Heading { slug, text, .. } if *slug == anchor || (self.notes_mode && crate::notes::parse::heading_key(&text.text) == key)))?;
                self.scroll_to = self.tops.get(i).map(|y| y - MARGIN * 0.5);
                ctx.request_repaint();
                None
            }
            Target::Web(u) => {
                ctx.open_url(egui::OpenUrl::new_tab(u));
                None
            }
            Target::Refused(msg) => Some(Action::Status(msg)),
            // In a vault, Markdown links find notes the way Obsidian does.
            Target::Local(_) if self.notes_mode => Some(Action::Note { link: url.to_owned(), markdown: true }),
            Target::Local(target) => Some(local_link(&target)),
        }
    }
}

/// A link to a local file: text opens in Strata; anything else (an app, a
/// script, an image) is shown in the file manager rather than launched.
pub fn local_link(target: &Path) -> Action {
    if !target.exists() {
        return Action::Status(format!("Link target not found: {}", target.display()));
    }
    let text_like = target.is_file()
        && std::fs::File::open(target)
            .and_then(|f| {
                let mut head = Vec::new();
                std::io::Read::read_to_end(&mut std::io::Read::take(f, 8000), &mut head).map(|_| head)
            })
            .is_ok_and(|b| !b.contains(&0));
    if text_like { Action::Open(target.to_path_buf()) } else { Action::Reveal(target.to_path_buf()) }
}

// ---- whole documents (the Help viewer) ---------------------------------------------

/// Draws a short document in full, in the current color scheme. Unlike the
/// preview it lays out every block (help pages are small) and reports where
/// each heading landed, for anchors and search hits.
#[derive(Default)]
pub struct DocRender {
    code: HashMap<u64, Option<Arc<Vec<Span>>>>,
    code_theme: String,
    font_attempt: u64,
}

/// What a document frame produced: a clicked link, and (slug, y) for every
/// top-level heading, y measured from the top of the content.
pub struct DocFrame {
    pub clicked: Option<String>,
    pub headings: Vec<(String, f32)>,
}

impl DocRender {
    /// Draw `blocks` at `size` points, at most `MAX_WIDTH` wide, into `ui`
    /// (normally the inside of a scroll area).
    pub fn show(&mut self, ui: &mut Ui, hl: &Highlighter, size: f32, blocks: &[TopBlock]) -> DocFrame {
        let ctx = ui.ctx().clone();
        let pal = Palette::new(hl);
        if self.code_theme != hl.theme_key() {
            self.code.clear();
            self.code_theme = hl.theme_key();
        }
        let (family, variable) = body_font(&ctx, &mut self.font_attempt);
        let full = ui.available_width();
        let width = (full - 2.0 * MARGIN).clamp(60.0, MAX_WIDTH);
        let origin = ui.max_rect().min;
        let rect = Rect::from_min_size(origin + vec2(((full - width) / 2.0).max(8.0), MARGIN * 0.75), vec2(width, 1e7));
        let mut child = ui.new_child(UiBuilder::new().max_rect(rect).layout(Layout::top_down(Align::Min)).id_salt("doc-blocks"));
        let mut d = Draw { pal: &pal, family, variable, size, theme: hl.theme_name(), code: &mut self.code, clicked: None, small: size * 0.9, notes: None, depth: 0, run: false, run_clicked: None };
        let mut headings = Vec::new();
        for (i, b) in blocks.iter().enumerate() {
            if i > 0 {
                child.add_space(size * 0.9);
            }
            if let Block::Heading { slug, .. } = &b.block {
                headings.push((slug.clone(), child.cursor().top() - origin.y));
            }
            block(&mut child, &mut d, &b.block, 0, false, i);
        }
        let clicked = d.clicked.take();
        let used = child.min_rect();
        ui.set_min_size(vec2(full, used.bottom() - origin.y + MARGIN * 2.0));
        DocFrame { clicked, headings }
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
        assert!(matches!(&blocks[6], Block::Code { lang, code, run: Some(_) } if lang == "rust" && code == "fn main() {}"));
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
    fn html_export_carries_no_scripts() {
        let md = "# T\n\n<script>alert(1)</script>\n\n<div onclick=\"x()\">hi <b>there</b></div>\n\nText <img src=x onerror=alert(2)> and <iframe src=\"https://evil.example\"></iframe>.\n\n[click](javascript:alert(3)) [ok](https://example.com) ![p](javascript:alert(4))\n\n<a href=\"jav&#x61;script:alert(5)\">a</a>\n";
        let html = to_html_fragment(md);
        for bad in ["<script", "onclick", "onerror", "<iframe", "javascript:", "jav&#x61;script"] {
            assert!(!html.to_ascii_lowercase().contains(bad), "{bad} in {html}");
        }
        assert!(html.contains("<b>there</b>") && html.contains("<div>"), "harmless HTML stays: {html}");
        assert!(html.contains("href=\"https://example.com\""), "{html}");
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"), "a script shows as text: {html}");
        // Copy as HTML and printing use the same path.
        assert!(!crate::print::markdown_html(md, "t", &Default::default()).contains("<script"));
    }

    #[test]
    fn exported_alerts_are_styled() {
        let html = to_html_document("> [!WARNING]\n> Careful.\n\n> [!TIP]\n> Nice.\n", "x");
        // Every class pulldown-cmark puts on an alert must have a rule in the page's CSS.
        for kind in ["note", "tip", "important", "warning", "caution"] {
            assert!(EXPORT_CSS.contains(&format!(".markdown-alert-{kind}")), "no style for {kind}");
        }
        assert!(html.contains("<blockquote class=\"markdown-alert-warning\">"), "{html}");
        assert!(EXPORT_CSS.contains("content:\"Warning\""), "alerts get their title");
    }

    #[test]
    fn code_language_label_never_covers_the_code() {
        let long = "let x = [".to_owned() + &"item, ".repeat(60) + "];";
        let src = format!("```rust\n{long}\n```\n\n```rust\nshort();\n```\n");
        let mut doc = Document::from_buffer(Some(PathBuf::from("/tmp/label.md")), crate::buffer::Buffer::from_str(&src));
        doc.highlighter.set_syntax("Markdown");
        let mut preview = Preview::default();
        let ctx = egui::Context::default();
        let mut shapes = Vec::new();
        for _ in 0..3 {
            let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(500.0, 700.0))), ..Default::default() };
            let mut out = ctx.run_ui(raw, |ui| {
                preview.show(ui, &mut doc);
            });
            out.textures_delta.clear();
            shapes = out.shapes;
        }
        fn texts(shape: &egui::Shape, out: &mut Vec<(String, Vec<Rect>)>) {
            match shape {
                egui::Shape::Text(t) => out.push((t.galley.text().to_owned(), t.galley.rows.iter().map(|r| r.rect().translate(t.pos.to_vec2())).collect())),
                egui::Shape::Vec(v) => v.iter().for_each(|s| texts(s, out)),
                _ => {}
            }
        }
        let mut found = Vec::new();
        shapes.iter().for_each(|c| texts(&c.shape, &mut found));
        let labels: Vec<Rect> = found.iter().filter(|(t, _)| t == "rust").map(|(_, r)| r[0]).collect();
        assert_eq!(labels.len(), 2, "both blocks are labeled");
        for (code, label) in [(long.as_str(), labels[0]), ("short();", labels[1])] {
            let rows = &found.iter().find(|(t, _)| t.starts_with(&code[..5])).unwrap().1;
            assert!(!rows[0].intersects(label.shrink(0.5)), "label {label:?} overlaps {code:.12}… at {:?}", rows[0]);
        }
    }

    #[test]
    fn hidden_characters_show_as_markers_everywhere_in_the_preview() {
        let r = "\u{202E}";
        let src = format!("# Head{r}ing\n\nPara{r} with [li{r}nk](https://x.dev/{r}) and `co{r}de`.\n\n- item{r}\n\n| A{r} | B |\n|--|--|\n| c{r} | d |\n\n```rust{r}\nlet x{r} = 1;\n```\n\n<div>ht{r}ml</div>\n\nNote[^1].\n\n[^1]: foot{r}\n");
        let mut doc = Document::from_buffer(Some(PathBuf::from("/tmp/hidden.md")), crate::buffer::Buffer::from_str(&src));
        doc.highlighter.set_syntax("Markdown");
        let mut preview = Preview::default();
        let ctx = egui::Context::default();
        let mut shapes = Vec::new();
        for _ in 0..3 {
            let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(700.0, 900.0))), ..Default::default() };
            let mut out = ctx.run_ui(raw, |ui| {
                preview.show(ui, &mut doc);
            });
            out.textures_delta.clear();
            shapes = out.shapes;
        }
        let texts = drawn(&shapes);
        assert!(texts.iter().all(|t| !crate::hidden::any(t)), "raw control drawn: {texts:?}");
        for want in ["Heading", "Para", "link", "code", "item", "A", "c", "x", "html"] {
            assert!(texts.iter().any(|t| t.contains("RLO") && t.replace("\u{2009}RLO\u{2009}", "").contains(want)), "{want} isn't marked: {texts:?}");
        }
        assert!(texts.iter().any(|t| t == "rust[RLO]"), "the language label: {texts:?}");
        // The link under its marked text still resolves to the link.
        let d_inline = match &parse(&src)[1].block {
            Block::Paragraph(i) => i.clone(),
            _ => unreachable!(),
        };
        let mut job = LayoutJob::default();
        crate::hidden::append(&mut job, &d_inline.text, 0.0, TextFormat::default());
        let at = job.text.find('l').unwrap();
        let shown_char = job.text[..at].chars().count() + 3;
        let src_char = crate::hidden::source_index(&d_inline.text, shown_char);
        let byte = d_inline.text.char_indices().nth(src_char).unwrap().0;
        assert_eq!(d_inline.link_at(byte), Some("https://x.dev/\u{202E}"));
    }

    /// Every text drawn into `shapes`.
    fn drawn(shapes: &[egui::epaint::ClippedShape]) -> Vec<String> {
        fn walk(shape: &egui::Shape, out: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(t) => out.push(t.galley.text().to_owned()),
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
                _ => {}
            }
        }
        let mut out = Vec::new();
        shapes.iter().for_each(|c| walk(&c.shape, &mut out));
        out
    }

    #[test]
    fn run_buttons_only_in_the_live_preview_of_runnable_closed_blocks() {
        let src = "```bash\necho a\n```\n\n```rust\nfn main() {}\n```\n\n    indented\n\n```python\nprint(1)\n```\n\n```sh\nnever closed\n";
        let blocks = parse(src);
        let runs: Vec<Option<usize>> = blocks.iter().filter_map(|b| if let Block::Code { run, .. } = &b.block { Some(*run) } else { None }).collect();
        assert_eq!(runs, vec![Some(0), src.find("```rust"), None, src.find("```python"), None], "fenced and closed blocks know where they start");
        let ctx = egui::Context::default();
        let run_ui = |f: &mut dyn FnMut(&mut Ui)| {
            let mut shapes = Vec::new();
            for _ in 0..3 {
                let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(600.0, 900.0))), ..Default::default() };
                let mut out = ctx.run_ui(raw, |ui| f(ui));
                out.textures_delta.clear();
                shapes = out.shapes;
            }
            drawn(&shapes).into_iter().filter(|t| t == "▶ Run").count()
        };
        // The live preview: bash and python (closed) get one; rust, indented and unclosed don't.
        let mut doc = Document::from_buffer(Some(PathBuf::from("/tmp/run.md")), crate::buffer::Buffer::from_str(src));
        doc.highlighter.set_syntax("Markdown");
        let mut preview = Preview::default();
        assert_eq!(run_ui(&mut |ui| {
            preview.show(ui, &mut doc);
        }), 2);
        // The same Markdown as a help page or an AI answer: none.
        let mut render = DocRender::default();
        let hl = Highlighter::new();
        assert_eq!(run_ui(&mut |ui| {
            render.show(ui, &hl, 14.0, &blocks);
        }), 0);
    }

    #[test]
    fn notes_parse_wikilinks_tags_embeds_and_frontmatter() {
        let src = "---\ntags: [a]\n---\nSee [[Other#Part|shown]], #tag_with_under/sub and `#code` or a#b.\n\n![[pic.png]]\n\nText ![[inline.png]] here\n";
        let blocks = top_note(src);
        assert!(matches!(&blocks[0], Block::Code { lang, code, run: None } if lang == "yaml" && code == "tags: [a]"), "{:?}", blocks[0]);
        let Block::Paragraph(p) = &blocks[1] else { panic!("{:?}", blocks[1]) };
        assert_eq!(p.link_at(p.text.find("shown").unwrap()), Some(format!("{NOTE_LINK}Other#Part").as_str()));
        let tags: Vec<&str> = p.runs.iter().filter(|r| r.style.tag).map(|r| &p.text[r.range.clone()]).collect();
        assert_eq!(tags, ["#tag_with_under/sub"], "whole runs, not in code or words");
        assert_eq!(p.link_at(p.text.find("#tag").unwrap()), Some(format!("{TAG_LINK}tag_with_under/sub").as_str()));
        assert_eq!(blocks[2], Block::Embed { target: "pic.png".into(), wiki: true }, "an embed alone is a block");
        assert!(matches!(&blocks[3], Block::Paragraph(_)), "one inside text stays inline");
        // Other Markdown is read as before: no wikilinks, frontmatter as text.
        assert!(!matches!(top("[[x]]\n").first(), Some(Block::Embed { .. })));
        assert!(matches!(&top("---\ntags: [a]\n---\n")[0], Block::Rule));
    }

    fn top_note(src: &str) -> Vec<Block> {
        parse_note(src).into_iter().map(|b| b.block).collect()
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
        // Other local files are shown in the file manager, never launched.
        std::fs::write(dir.join("tool.bin"), b"\x7fELF\0\0\x01").unwrap();
        assert_eq!(preview.follow(&ctx, "tool.bin", Some(&path)), Some(Action::Reveal(dir.join("tool.bin"))));
        // Schemes other than http(s)/mailto are refused with the target named.
        match preview.follow(&ctx, "file:///Applications/Calculator.app", Some(&path)) {
            Some(Action::Status(s)) => assert!(s.contains("file:///Applications/Calculator.app"), "{s}"),
            a => panic!("{a:?}"),
        }
        assert!(matches!(preview.follow(&ctx, "smb://host/share", Some(&path)), Some(Action::Status(_))));
        assert_eq!(preview.follow(&ctx, "https://example.com", Some(&path)), None, "web links open in the browser");
        // Editing re-parses.
        doc.editor.insert_text("Intro\n\n");
        assert!(preview.refresh(&mut doc, 10.0));
        assert_eq!(preview.blocks().len(), 13);
        std::fs::remove_dir_all(dir).ok();
    }
}
