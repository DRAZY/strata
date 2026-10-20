//! Word completion popup: words from the buffer that start with what you're
//! typing, shown under the caret, plus snippets whose trigger starts with it
//! (labelled with their description), and a language server's completions
//! first when one answers (see `lsp_ui`). Up/Down choose, Enter or Tab
//! insert, Esc closes.

use crate::document::Document;
use crate::lsp::edit::TextEdit;
use crate::snippets::{Library, Snippet};
use egui::{Color32, Key, Modifiers, Sense, pos2, vec2};
use serde_json::Value;

const MAX_ITEMS: usize = 12;
/// A server can offer thousands; the list shows the best this many.
const MAX_SERVER_ITEMS: usize = 100;
/// Room kept between the list (with its frame) and the window's edges.
const POPUP_EDGE: f32 = 16.0;

/// `note` in at most `room` chars (cut with …); None when there's no room for any of it.
fn fit(note: &str, room: usize) -> Option<String> {
    let n = note.chars().count();
    if n <= room {
        return Some(note.to_owned());
    }
    (room >= 4).then(|| format!("{}…", note.chars().take(room - 1).collect::<String>()))
}

#[derive(Default)]
pub struct Completion {
    /// Editor (view id) the list belongs to; None when closed.
    owner: Option<egui::Id>,
    items: Vec<Item>,
    selected: usize,
    /// (text version, cursor) the list was computed for.
    key: (u64, usize),
    /// A snippet chosen from the list with the prefix it replaces; the app
    /// expands it (it owns the field session).
    accepted: Option<(Snippet, String)>,
    /// A server item chosen from the list; the app inserts it (it knows the server's positions).
    accepted_server: Option<ServerItem>,
    /// The server's answer for the word being typed.
    server: Option<ServerList>,
    /// Esc closed the list (an answer still on its way shouldn't reopen it).
    dismissed: bool,
}

/// A completion from a language server.
#[derive(Clone, Debug, PartialEq)]
pub struct ServerItem {
    pub label: String,
    /// Matched against what's typed (`filterText`, else the label).
    pub filter: String,
    /// Orders the list (`sortText`, else the label).
    pub sort: String,
    pub detail: Option<String>,
    pub documentation: Option<String>,
    pub kind: Option<u8>,
    /// The text to insert: a snippet (`$1`, `${2:x}`) when `snippet`.
    pub insert: String,
    pub snippet: bool,
    /// The range `insert` replaces, in the server's positions when it was asked.
    pub range: Option<(crate::lsp::position::Pos, crate::lsp::position::Pos)>,
    /// Other edits that come with it (an import at the top).
    pub additional: Vec<TextEdit>,
    /// The item as the server sent it, for `completionItem/resolve`.
    pub raw: Value,
    pub resolved: bool,
}

impl ServerItem {
    pub fn from_json(v: &Value) -> Option<Self> {
        let label = v.get("label")?.as_str()?.to_owned();
        let str_of = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
        let edit = v.get("textEdit").and_then(TextEdit::from_json);
        let insert = edit.as_ref().map(|e| e.text.clone()).or_else(|| str_of("insertText")).unwrap_or_else(|| label.clone());
        let mut item = Self {
            filter: str_of("filterText").unwrap_or_else(|| label.clone()),
            sort: str_of("sortText").unwrap_or_else(|| label.clone()),
            detail: None,
            documentation: None,
            kind: v.get("kind").and_then(Value::as_u64).map(|k| k.min(255) as u8),
            insert,
            snippet: v.get("insertTextFormat").and_then(Value::as_u64) == Some(2),
            range: edit.map(|e| e.range),
            additional: Vec::new(),
            raw: v.clone(),
            resolved: false,
            label,
        };
        item.take_details(v);
        Some(item)
    }

    /// Detail, documentation and extra edits from the item (or its resolved form).
    pub fn take_details(&mut self, v: &Value) {
        if let Some(d) = v.get("detail").and_then(Value::as_str).filter(|d| !d.is_empty()) {
            self.detail = Some(d.to_owned());
        }
        let doc = match v.get("documentation") {
            Some(Value::String(s)) => Some(s.clone()),
            Some(o) => o.get("value").and_then(Value::as_str).map(str::to_owned),
            None => None,
        };
        if let Some(doc) = doc.filter(|d| !d.trim().is_empty()) {
            self.documentation = Some(doc);
        }
        if let Some(a) = v.get("additionalTextEdits").and_then(Value::as_array) {
            self.additional = a.iter().filter_map(TextEdit::from_json).collect();
        }
    }

    /// A short word for the kind, shown when there's no detail.
    fn kind_name(&self) -> Option<&'static str> {
        Some(match self.kind? {
            2 => "method",
            3 => "function",
            4 => "constructor",
            5 => "field",
            6 => "variable",
            7 => "class",
            8 => "interface",
            9 => "module",
            10 => "property",
            13 => "enum",
            14 => "keyword",
            15 => "snippet",
            20 => "variant",
            21 => "constant",
            22 => "struct",
            25 => "type parameter",
            _ => return None,
        })
    }
}

/// A server's completions for the word starting at `start` in one editor.
pub struct ServerList {
    pub view: egui::Id,
    pub start: usize,
    pub items: Vec<ServerItem>,
    /// The server may have more: ask again as the word grows.
    pub incomplete: bool,
}

enum Item {
    Word(String),
    Snippet(Snippet),
    /// Index into `ServerList::items`.
    Server(usize),
}

impl Completion {
    pub fn is_open(&self) -> bool {
        self.owner.is_some()
    }

    pub fn close(&mut self) {
        self.owner = None;
        self.items.clear();
    }

    fn text<'a>(&'a self, item: &'a Item) -> &'a str {
        match item {
            Item::Word(w) => w,
            Item::Snippet(s) => &s.trigger,
            Item::Server(i) => self.server.as_ref().map_or("", |l| l.items[*i].label.as_str()),
        }
    }

    /// The server's list, if it's for the word at `d`'s cursor: the cursor is
    /// on the line where the word starts, after it, with only word chars between.
    fn server_prefix(&self, d: &Document) -> Option<String> {
        let list = self.server.as_ref().filter(|l| l.view == d.view.id)?;
        let ed = &d.editor;
        let head = ed.primary().head;
        let b = &ed.buffer;
        if head < list.start || head > b.len_chars() || head - list.start > 256 || b.line_of(head) != b.line_of(list.start) {
            return None;
        }
        let prefix = b.slice(list.start..head);
        prefix.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '$').then_some(prefix)
    }

    /// Server items matching `prefix`, best first: those starting with it
    /// (in the server's order), then looser matches.
    fn server_matches(&self, prefix: &str) -> Vec<usize> {
        let Some(list) = &self.server else { return Vec::new() };
        let mut order: Vec<usize> = (0..list.items.len()).collect();
        order.sort_by(|&a, &b| list.items[a].sort.cmp(&list.items[b].sort));
        if prefix.is_empty() {
            order.truncate(MAX_SERVER_ITEMS);
            return order;
        }
        let lower = prefix.to_lowercase();
        let (mut starts, rest): (Vec<usize>, Vec<usize>) = order.into_iter().partition(|&i| list.items[i].filter.to_lowercase().starts_with(&lower));
        let keys: Vec<&str> = rest.iter().map(|&i| list.items[i].filter.as_str()).collect();
        let ranked = crate::fuzzy::rank(prefix, &keys, |s| s, MAX_SERVER_ITEMS);
        starts.extend(ranked.into_iter().map(|(k, _, _)| rest[k]));
        starts.truncate(MAX_SERVER_ITEMS);
        starts
    }

    /// Show completions for the word before the cursor. `explicit` (Ctrl+Space)
    /// inserts a single candidate right away.
    pub fn open(&mut self, d: &mut Document, explicit: bool, snippets: &Library) {
        let server: Vec<usize> = match self.server_prefix(d) {
            Some(prefix) => self.server_matches(&prefix),
            None => {
                self.server = None;
                Vec::new()
            }
        };
        let labels: Vec<&str> = server.iter().map(|&i| self.server.as_ref().map_or("", |l| l.items[i].label.as_str())).collect();
        let words: Vec<String> = d.editor.completions(MAX_ITEMS).into_iter().filter(|w| !labels.contains(&w.as_str())).collect();
        let (exact, partial): (Vec<Snippet>, Vec<Snippet>) = match d.editor.word_prefix_at_cursor() {
            Some((start, prefix)) => {
                let scopes = d.highlighter.scope_at(&d.editor.buffer, start);
                snippets.completions(&prefix, &scopes).into_iter().cloned().partition(|s| s.trigger == prefix)
            }
            None => Default::default(),
        };
        // A trigger typed in full comes first, then the server's items,
        // other snippets after the words.
        let mut items: Vec<Item> = exact.into_iter().map(Item::Snippet).collect();
        items.extend(server.into_iter().map(Item::Server));
        items.extend(words.into_iter().map(Item::Word));
        items.extend(partial.into_iter().map(Item::Snippet));
        items.truncate(MAX_ITEMS + MAX_SERVER_ITEMS);
        self.key = (d.editor.version(), d.editor.primary().head);
        if explicit && items.len() == 1 && !matches!(items[0], Item::Server(_)) {
            self.items = items;
            self.accept(d, 0);
            self.close();
            return;
        }
        if items.is_empty() {
            self.close();
            return;
        }
        // An item chosen with the arrows stays chosen when the list is
        // refreshed (a server's answer arriving); otherwise the best comes first.
        let was = self.items.get(self.selected).filter(|_| self.is_open() && self.selected > 0).map(|i| self.text(i).to_owned());
        self.owner = Some(d.view.id);
        self.items = items;
        self.selected = was.and_then(|w| self.items.iter().position(|i| self.text(i) == w)).unwrap_or(0);
    }

    /// After typing: open when a word of 2+ chars is being typed (any length
    /// when a server answered for it), refresh or close when the text or
    /// cursor moved.
    pub fn after_edit(&mut self, d: &mut Document, typed_word_char: bool, snippets: &Library) {
        let key = (d.editor.version(), d.editor.primary().head);
        if key == self.key {
            return;
        }
        let served = self.server_prefix(d).is_some();
        if !served {
            self.server = None;
        }
        let long_enough = served || d.editor.word_prefix_at_cursor().is_some_and(|(_, p)| p.chars().count() >= 2);
        if (self.is_open() || typed_word_char) && long_enough && d.editor.selections().len() < 50 {
            self.open(d, false, snippets);
        } else {
            self.close();
            self.key = key;
        }
    }

    /// A server's answer for the word starting at `start`: show it (with
    /// the words and snippets) if it's still for the word being typed.
    pub fn set_server_items(&mut self, d: &mut Document, list: ServerList, snippets: &Library) {
        self.server = Some(list);
        if self.server_prefix(d).is_none() {
            self.server = None;
            return;
        }
        self.open(d, false, snippets);
    }

    /// The word start and whether to ask again, for the server's list in this editor.
    pub fn server_state(&self, view: egui::Id) -> Option<(usize, bool)> {
        self.server.as_ref().filter(|l| l.view == view).map(|l| (l.start, l.incomplete))
    }

    /// The highlighted server item, if it needs resolving (its index and raw form).
    pub fn selected_unresolved(&self) -> Option<(usize, Value)> {
        let Item::Server(i) = self.items.get(self.selected)? else { return None };
        let item = &self.server.as_ref()?.items[*i];
        (!item.resolved).then(|| (*i, item.raw.clone()))
    }

    /// The server's resolved form of item `i` (its label confirms it's the same item).
    pub fn resolved(&mut self, i: usize, v: &Value) {
        if let Some(item) = self.server.as_mut().and_then(|l| l.items.get_mut(i))
            && v.get("label").and_then(Value::as_str) == Some(item.label.as_str())
        {
            item.take_details(v);
            item.resolved = true;
        }
    }

    pub fn mark_resolved(&mut self, i: usize) {
        if let Some(item) = self.server.as_mut().and_then(|l| l.items.get_mut(i)) {
            item.resolved = true;
        }
    }

    /// Navigation keys, taken before the editor sees them. Returns true if handled.
    pub fn keys(&mut self, ctx: &egui::Context, d: &mut Document) -> bool {
        if self.owner != Some(d.view.id) {
            return false;
        }
        let (up, down, accept, esc) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::Enter) || i.consume_key(Modifiers::NONE, Key::Tab),
                i.consume_key(Modifiers::NONE, Key::Escape),
            )
        });
        let n = self.items.len();
        if up {
            self.selected = (self.selected + n - 1) % n;
        }
        if down {
            self.selected = (self.selected + 1) % n;
        }
        if accept {
            self.accept(d, self.selected);
            d.view.reveal();
            self.close();
            self.key = (d.editor.version(), d.editor.primary().head);
        }
        if esc {
            self.close();
            self.server = None;
            self.dismissed = true;
            self.key = (d.editor.version(), d.editor.primary().head);
        }
        up || down || accept || esc
    }

    /// Insert a word, or hand a snippet or server item to the app (`take_snippet`, `take_server_item`).
    fn accept(&mut self, d: &mut Document, i: usize) {
        match &self.items[i] {
            Item::Word(w) => d.editor.complete(w),
            Item::Snippet(s) => {
                let prefix = d.editor.word_prefix_at_cursor().map(|(_, p)| p).unwrap_or_default();
                self.accepted = Some((s.clone(), prefix));
            }
            Item::Server(k) => {
                self.accepted_server = self.server.take().map(|mut l| l.items.swap_remove(*k));
            }
        }
    }

    pub fn take_snippet(&mut self) -> Option<(Snippet, String)> {
        self.accepted.take()
    }

    /// Esc closed the list since the last call.
    pub fn take_dismissed(&mut self) -> bool {
        std::mem::take(&mut self.dismissed)
    }

    pub fn take_server_item(&mut self) -> Option<ServerItem> {
        self.accepted_server.take()
    }

    #[cfg(test)]
    pub fn has_snippet(&self, trigger: &str) -> bool {
        self.items.iter().any(|i| matches!(i, Item::Snippet(s) if s.trigger == trigger))
    }

    /// The labels shown, in order (tests).
    #[cfg(test)]
    pub fn labels(&self) -> Vec<String> {
        self.items.iter().map(|i| self.text(i).to_owned()).collect()
    }

    /// Choose item `i` as a click would (tests).
    #[cfg(test)]
    pub fn choose(&mut self, d: &mut Document, i: usize) {
        self.accept(d, i);
        self.close();
    }

    /// Draw the list under the caret; a click inserts that word.
    pub fn show(&mut self, ctx: &egui::Context, d: &mut Document) {
        if self.owner != Some(d.view.id) {
            return;
        }
        let Some(caret) = d.view.caret_rect() else { return };
        let font = egui::FontId::monospace(d.view.font_size.max(12.0) - 1.0);
        let row_h = font.size + 8.0;
        let mut chosen = None;
        // Below the caret, or above it when there's more room there: never over its line.
        let id = egui::Id::new("completions");
        let screen = ctx.content_rect();
        let spacing = ctx.global_style().spacing.item_spacing.y;
        let wanted = self.items.len().min(MAX_ITEMS) as f32 * (row_h + spacing) + 16.0;
        let (below, above) = (screen.bottom() - caret.bottom() - 4.0, caret.top() - screen.top() - 4.0);
        let up = wanted > below && above > below;
        let room = if up { above } else { below };
        let (pivot, mut at) = if up { (egui::Align2::LEFT_BOTTOM, caret.left_top() + vec2(-4.0, -2.0)) } else { (egui::Align2::LEFT_TOP, caret.left_bottom() + vec2(-4.0, 2.0)) };
        let selected = self.selected;
        let texts: Vec<String> = self.items.iter().map(|item| self.text(item).to_owned()).collect();
        // Width: the longest name and note, but never wider than the window;
        // notes are cut short (…) to fit, and the list moves left to stay inside.
        let char_w = ctx.fonts_mut(|f| f.glyph_width(&font, 'M'));
        // Hidden characters draw as markers a few letters wide (see `hidden::append`).
        let text_w = texts.iter().map(|t| crate::hidden::plain(t).chars().count() + 2 * usize::from(crate::hidden::any(t))).max().unwrap_or(8) as f32 * char_w;
        let max_w = (screen.width() - 2.0 * POPUP_EDGE).max(140.0);
        let note_room = ((max_w - text_w - 48.0) / char_w).floor().max(0.0) as usize;
        let notes: Vec<Option<String>> = self.items.iter().map(|item| self.note(item).and_then(|n| fit(&n, note_room))).collect();
        let note_w = notes.iter().flatten().map(|n| crate::hidden::plain(n).chars().count()).max().map_or(0.0, |n| n as f32 * char_w + 24.0);
        let width = (text_w + note_w + 24.0).max(140.0).min(max_w);
        at.x = at.x.min(screen.right() - POPUP_EDGE - width).max(screen.left() + POPUP_EDGE - 8.0);
        let doc = match self.items.get(selected) {
            Some(Item::Server(i)) => self.server.as_ref().and_then(|l| l.items[*i].documentation.clone()),
            _ => None,
        };
        let list = egui::Area::new(id)
            .order(egui::Order::Foreground)
            .pivot(pivot)
            .fixed_pos(at)
            .constrain(false)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).inner_margin(4.0).show(ui, |ui| {
                    egui::ScrollArea::vertical().max_height((room - 16.0).max(row_h).min(MAX_ITEMS as f32 * (row_h + spacing))).show(ui, |ui| {
                    let dim = ui.visuals().weak_text_color();
                    for (i, text) in texts.iter().enumerate() {
                        let (rect, resp) = ui.allocate_exact_size(vec2(width, row_h), Sense::click());
                        if i == self.selected {
                            ui.painter().rect_filled(rect, 3.0, ui.visuals().selection.bg_fill);
                        }
                        let (job, note) = row_jobs(text, notes[i].as_deref(), &font, ui.visuals().text_color(), dim);
                        let g = ui.painter().layout_job(job);
                        ui.painter().galley(pos2(rect.left() + 8.0, rect.center().y - g.size().y / 2.0), g, ui.visuals().text_color());
                        if let Some(job) = note {
                            let g = ui.painter().layout_job(job);
                            ui.painter().galley(pos2(rect.right() - 8.0 - g.size().x, rect.center().y - g.size().y / 2.0), g, dim);
                        }
                        // Accepting inserts the server's text as it is; say what's in it first.
                        let resp = if crate::hidden::any(text) { resp.on_hover_text("This suggestion contains hidden or direction-changing characters, shown as boxes. Accepting it inserts them as they are.") } else { resp };
                        if resp.clicked() {
                            chosen = Some(i);
                        }
                        if i == selected {
                            resp.scroll_to_me(None);
                        }
                    }
                    });
                });
            });
        // The highlighted server item's documentation, beside the list.
        if let Some(doc) = doc {
            let r = list.response.rect;
            let right = screen.right() - r.right() > 260.0;
            let (pivot, at) = if right { (egui::Align2::LEFT_TOP, r.right_top() + vec2(4.0, 0.0)) } else { (egui::Align2::RIGHT_TOP, r.left_top() - vec2(4.0, 0.0)) };
            egui::Area::new(id.with("doc")).order(egui::Order::Foreground).pivot(pivot).fixed_pos(at).constrain(true).interactable(false).show(ctx, |ui| {
                egui::Frame::popup(ui.style()).inner_margin(8.0).show(ui, |ui| {
                    ui.set_max_width(360.0);
                    let text: String = doc.lines().take(14).collect::<Vec<_>>().join("\n");
                    ui.label(crate::hidden::widget(&text, egui::FontId::proportional(font.size), ui.visuals().text_color()));
                });
            });
        }
        if let Some(i) = chosen {
            self.accept(d, i);
            d.view.focus();
            self.close();
        }
    }

    /// The dim text at the right of a row.
    fn note(&self, item: &Item) -> Option<String> {
        match item {
            Item::Snippet(s) => Some(format!("{}  ⇥", s.label())),
            Item::Word(_) => None,
            Item::Server(i) => {
                let it = &self.server.as_ref()?.items[*i];
                let text = it.detail.clone().or_else(|| it.kind_name().map(str::to_owned))?;
                let first = text.lines().next().unwrap_or_default();
                Some(if first.chars().count() > 40 { format!("{}…", first.chars().take(39).collect::<String>()) } else { first.to_owned() })
            }
        }
    }
}

/// A completion row's name and note, laid out with hidden characters as
/// markers. A name holding any starts with a warning sign, since a server
/// could use them to pass off one identifier as another.
fn row_jobs(text: &str, note: Option<&str>, font: &egui::FontId, color: Color32, dim: Color32) -> (egui::text::LayoutJob, Option<egui::text::LayoutJob>) {
    let mut job = egui::text::LayoutJob::default();
    if crate::hidden::any(text) {
        let warn = crate::view::marker_color(2, color.r() as u32 + color.g() as u32 + color.b() as u32 <= 3 * 128).unwrap_or(color);
        job.append("⚠ ", 0.0, egui::TextFormat::simple(font.clone(), warn));
    }
    crate::hidden::append(&mut job, text, 0.0, egui::TextFormat::simple(font.clone(), color));
    (job, note.map(|n| crate::hidden::job(n, egui::TextFormat::simple(font.clone(), dim))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;
    use serde_json::json;

    #[test]
    fn rows_with_hidden_characters_are_marked_and_warned() {
        let font = egui::FontId::monospace(13.0);
        let (job, note) = row_jobs("is\u{202E}Admin", Some("fn is\u{2066}Admin()"), &font, Color32::WHITE, Color32::GRAY);
        assert_eq!(crate::hidden::shown(&job), "⚠ is\u{2009}RLO\u{2009}Admin");
        assert_eq!(crate::hidden::shown(&note.unwrap()), "fn is\u{2009}LRI\u{2009}Admin()");
        let (plain, _) = row_jobs("isAdmin", None, &font, Color32::WHITE, Color32::GRAY);
        assert_eq!(plain.text, "isAdmin", "no warning sign on clean names");
    }

    fn doc(text: &str, cursor: usize) -> Document {
        let mut d = Document::from_buffer(None, Buffer::from_str(text));
        d.editor.select_ranges(&[cursor..cursor], 0);
        d
    }

    #[test]
    fn opens_after_two_chars_and_inserts_the_choice() {
        let mut d = doc("alphabet alpine al", 18);
        let mut c = Completion::default();
        c.after_edit(&mut d, true, &Library::default());
        assert!(c.is_open());
        assert_eq!(c.items.len(), 2);
        c.selected = c.items.iter().position(|w| c.text(w) == "alpine").unwrap();
        c.accept(&mut d, c.selected);
        assert_eq!(d.editor.buffer.slice(0..d.editor.buffer.len_chars()), "alphabet alpine alpine");
    }

    #[test]
    fn explicit_with_one_match_completes_immediately() {
        let mut d = doc("unique un", 9);
        let mut c = Completion::default();
        c.open(&mut d, true, &Library::default());
        assert!(!c.is_open());
        assert_eq!(d.editor.buffer.slice(0..d.editor.buffer.len_chars()), "unique unique");
    }

    #[test]
    fn one_letter_does_not_pop_up() {
        let mut d = doc("apple a", 7);
        let mut c = Completion::default();
        c.after_edit(&mut d, true, &Library::default());
        assert!(!c.is_open());
    }

    #[test]
    fn snippets_join_the_list_only_in_their_scope() {
        let mut d = Document::from_buffer(Some("a.rs".into()), Buffer::from_str("impossible im"));
        d.editor.select_ranges(&[13..13], 0);
        let mut c = Completion::default();
        c.after_edit(&mut d, true, &Library::default());
        assert_eq!(c.labels(), ["impossible", "impl"]);
        c.accept(&mut d, 1);
        let (s, prefix) = c.take_snippet().unwrap();
        assert_eq!((s.trigger.as_str(), prefix.as_str()), ("impl", "im"));

        let mut py = Document::from_buffer(Some("a.py".into()), Buffer::from_str("im"));
        py.editor.select_ranges(&[2..2], 0);
        c.after_edit(&mut py, true, &Library::default());
        assert!(!c.is_open(), "no Rust snippets in Python");
    }

    fn items(labels: &[(&str, &str)]) -> Vec<ServerItem> {
        labels.iter().map(|(l, sort)| ServerItem::from_json(&json!({"label": l, "sortText": sort, "detail": format!("fn {l}()")})).unwrap()).collect()
    }

    #[test]
    fn server_items_come_first_filter_as_you_type_and_drop_duplicate_words() {
        let lib = Library::default();
        // "self." then the server answers for the empty word after the dot.
        let mut d = doc("let value = 1; self.", 20);
        let mut c = Completion::default();
        let view = d.view.id;
        c.set_server_items(&mut d, ServerList { view, start: 20, items: items(&[("value", "b"), ("vector", "a"), ("other", "c")]), incomplete: false }, &lib);
        assert!(c.is_open(), "shown even with nothing typed yet");
        assert_eq!(c.labels(), ["vector", "value", "other"], "the server's order");
        // Typing narrows it; "value" from the buffer isn't listed twice.
        d.editor.insert_text("va");
        c.after_edit(&mut d, true, &lib);
        assert_eq!(c.labels(), ["value"]);
        assert_eq!(c.note(&c.items[0]).as_deref(), Some("fn value()"));
        // Choosing it hands it to the app.
        c.choose(&mut d, 0);
        assert_eq!(c.take_server_item().unwrap().label, "value");
        // Moving to another line drops the server's list.
        let mut d = doc("abc\nxy", 6);
        let view = d.view.id;
        c.set_server_items(&mut d, ServerList { view, start: 2, items: items(&[("abc", "a")]), incomplete: false }, &lib);
        assert!(!c.is_open() && c.server.is_none(), "an answer for a word elsewhere is dropped");
    }

    #[test]
    fn server_items_parse_text_edits_snippets_and_resolve() {
        let v = json!({
            "label": "push",
            "kind": 2,
            "filterText": "push",
            "insertTextFormat": 2,
            "textEdit": {"range": {"start": {"line": 0, "character": 4}, "end": {"line": 0, "character": 6}}, "newText": "push(${1:value})$0"},
        });
        let mut it = ServerItem::from_json(&v).unwrap();
        assert!(it.snippet);
        assert_eq!(it.insert, "push(${1:value})$0");
        assert_eq!(it.range.unwrap().0, crate::lsp::position::Pos::new(0, 4));
        assert_eq!(it.kind_name(), Some("method"));
        it.take_details(&json!({"detail": "fn push(&mut self, value: T)", "documentation": {"kind": "markdown", "value": "Appends."}, "additionalTextEdits": [{"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}}, "newText": "use x;\n"}]}));
        assert_eq!((it.detail.as_deref(), it.documentation.as_deref(), it.additional.len()), (Some("fn push(&mut self, value: T)"), Some("Appends."), 1));
        // insertText without a textEdit, and a bare label.
        assert_eq!(ServerItem::from_json(&json!({"label": "a", "insertText": "b"})).unwrap().insert, "b");
        assert_eq!(ServerItem::from_json(&json!({"label": "a"})).unwrap().insert, "a");
        assert!(ServerItem::from_json(&json!({"kind": 1})).is_none());
    }
}
