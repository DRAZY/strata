//! Word completion popup: words from the buffer that start with what you're
//! typing, shown under the caret, plus snippets whose trigger starts with it
//! (labelled with their description). Up/Down choose, Enter or Tab insert,
//! Esc closes.

use crate::document::Document;
use crate::snippets::{Library, Snippet};
use egui::{Key, Modifiers, Sense, vec2};

const MAX_ITEMS: usize = 12;

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
}

enum Item {
    Word(String),
    Snippet(Snippet),
}

impl Item {
    fn text(&self) -> &str {
        match self {
            Item::Word(w) => w,
            Item::Snippet(s) => &s.trigger,
        }
    }
}

impl Completion {
    pub fn is_open(&self) -> bool {
        self.owner.is_some()
    }

    pub fn close(&mut self) {
        self.owner = None;
        self.items.clear();
    }

    /// Show completions for the word before the cursor. `explicit` (Ctrl+Space)
    /// inserts a single candidate right away.
    pub fn open(&mut self, d: &mut Document, explicit: bool, snippets: &Library) {
        let words = d.editor.completions(MAX_ITEMS);
        let (exact, partial): (Vec<Snippet>, Vec<Snippet>) = match d.editor.word_prefix_at_cursor() {
            Some((start, prefix)) => {
                let scopes = d.highlighter.scope_at(&d.editor.buffer, start);
                snippets.completions(&prefix, &scopes).into_iter().cloned().partition(|s| s.trigger == prefix)
            }
            None => Default::default(),
        };
        // A trigger typed in full comes first, other snippets after the words.
        let mut items: Vec<Item> = exact.into_iter().map(Item::Snippet).collect();
        items.extend(words.into_iter().map(Item::Word));
        items.extend(partial.into_iter().map(Item::Snippet));
        items.truncate(MAX_ITEMS);
        self.key = (d.editor.version(), d.editor.primary().head);
        if explicit && items.len() == 1 {
            self.items = items;
            self.accept(d, 0);
            self.close();
            return;
        }
        if items.is_empty() {
            self.close();
            return;
        }
        self.owner = Some(d.view.id);
        self.items = items;
        self.selected = 0;
    }

    /// After typing: open when a word of 2+ chars is being typed, refresh or
    /// close when the text or cursor moved.
    pub fn after_edit(&mut self, d: &mut Document, typed_word_char: bool, snippets: &Library) {
        let key = (d.editor.version(), d.editor.primary().head);
        if key == self.key {
            return;
        }
        let long_enough = d.editor.word_prefix_at_cursor().is_some_and(|(_, p)| p.chars().count() >= 2);
        if (self.is_open() || typed_word_char) && long_enough && d.editor.selections().len() < 50 {
            self.open(d, false, snippets);
        } else {
            self.close();
            self.key = key;
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
            self.key = (d.editor.version(), d.editor.primary().head);
        }
        up || down || accept || esc
    }

    /// Insert a word, or hand a snippet to the app (`take_snippet`).
    fn accept(&mut self, d: &mut Document, i: usize) {
        match &self.items[i] {
            Item::Word(w) => d.editor.complete(w),
            Item::Snippet(s) => {
                let prefix = d.editor.word_prefix_at_cursor().map(|(_, p)| p).unwrap_or_default();
                self.accepted = Some((s.clone(), prefix));
            }
        }
    }

    pub fn take_snippet(&mut self) -> Option<(Snippet, String)> {
        self.accepted.take()
    }

    #[cfg(test)]
    pub fn has_snippet(&self, trigger: &str) -> bool {
        self.items.iter().any(|i| matches!(i, Item::Snippet(s) if s.trigger == trigger))
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
        let wanted = self.items.len() as f32 * (row_h + spacing) + 16.0;
        let (below, above) = (screen.bottom() - caret.bottom() - 4.0, caret.top() - screen.top() - 4.0);
        let up = wanted > below && above > below;
        let room = if up { above } else { below };
        let (pivot, at) = if up { (egui::Align2::LEFT_BOTTOM, caret.left_top() + vec2(-4.0, -2.0)) } else { (egui::Align2::LEFT_TOP, caret.left_bottom() + vec2(-4.0, 2.0)) };
        let selected = self.selected;
        egui::Area::new(id)
            .order(egui::Order::Foreground)
            .pivot(pivot)
            .fixed_pos(at)
            .constrain(false)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).inner_margin(4.0).show(ui, |ui| {
                    egui::ScrollArea::vertical().max_height((room - 16.0).max(row_h)).show(ui, |ui| {
                    let char_w = font.size * 0.62;
                    let text_w = self.items.iter().map(|w| w.text().chars().count()).max().unwrap_or(8) as f32 * char_w;
                    let note = |item: &Item| match item {
                        Item::Snippet(s) => Some(format!("{}  ⇥", s.label())),
                        Item::Word(_) => None,
                    };
                    let note_w = self.items.iter().filter_map(note).map(|n| n.chars().count()).max().map_or(0.0, |n| n as f32 * char_w + 24.0);
                    let width = text_w + note_w + 24.0;
                    let dim = ui.visuals().weak_text_color();
                    for (i, item) in self.items.iter().enumerate() {
                        let (rect, resp) = ui.allocate_exact_size(vec2(width.max(140.0), row_h), Sense::click());
                        if i == self.selected {
                            ui.painter().rect_filled(rect, 3.0, ui.visuals().selection.bg_fill);
                        }
                        ui.painter().text(rect.left_center() + vec2(8.0, 0.0), egui::Align2::LEFT_CENTER, item.text(), font.clone(), ui.visuals().text_color());
                        if let Some(n) = note(item) {
                            ui.painter().text(rect.right_center() - vec2(8.0, 0.0), egui::Align2::RIGHT_CENTER, n, font.clone(), dim);
                        }
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
        if let Some(i) = chosen {
            self.accept(d, i);
            d.view.focus();
            self.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;

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
        c.selected = c.items.iter().position(|w| w.text() == "alpine").unwrap();
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
        let texts: Vec<&str> = c.items.iter().map(Item::text).collect();
        assert_eq!(texts, ["impossible", "impl"]);
        c.accept(&mut d, 1);
        let (s, prefix) = c.take_snippet().unwrap();
        assert_eq!((s.trigger.as_str(), prefix.as_str()), ("impl", "im"));

        let mut py = Document::from_buffer(Some("a.py".into()), Buffer::from_str("im"));
        py.editor.select_ranges(&[2..2], 0);
        c.after_edit(&mut py, true, &Library::default());
        assert!(!c.is_open(), "no Rust snippets in Python");
    }
}
