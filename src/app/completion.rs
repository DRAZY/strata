//! Word completion popup: words from the buffer that start with what you're
//! typing, shown under the caret. Up/Down choose, Enter or Tab insert, Esc closes.

use crate::document::Document;
use egui::{Key, Modifiers, Sense, vec2};

const MAX_ITEMS: usize = 12;

#[derive(Default)]
pub struct Completion {
    /// Editor (view id) the list belongs to; None when closed.
    owner: Option<egui::Id>,
    items: Vec<String>,
    selected: usize,
    /// (text version, cursor) the list was computed for.
    key: (u64, usize),
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
    pub fn open(&mut self, d: &mut Document, explicit: bool) {
        let items = d.editor.completions(MAX_ITEMS);
        self.key = (d.editor.version(), d.editor.primary().head);
        if explicit && items.len() == 1 {
            d.editor.complete(&items[0]);
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
    pub fn after_edit(&mut self, d: &mut Document, typed_word_char: bool) {
        let key = (d.editor.version(), d.editor.primary().head);
        if key == self.key {
            return;
        }
        let long_enough = d.editor.word_prefix_at_cursor().is_some_and(|(_, p)| p.chars().count() >= 2);
        if (self.is_open() || typed_word_char) && long_enough && d.editor.selections().len() < 50 {
            self.open(d, false);
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
            let word = self.items[self.selected].clone();
            d.editor.complete(&word);
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

    /// Draw the list under the caret; a click inserts that word.
    pub fn show(&mut self, ctx: &egui::Context, d: &mut Document) {
        if self.owner != Some(d.view.id) {
            return;
        }
        let Some(caret) = d.view.caret_rect() else { return };
        let font = egui::FontId::monospace(d.view.font_size.max(12.0) - 1.0);
        let row_h = font.size + 8.0;
        let mut chosen = None;
        egui::Area::new(egui::Id::new("completions"))
            .order(egui::Order::Foreground)
            .fixed_pos(caret.left_bottom() + vec2(-4.0, 2.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).inner_margin(4.0).show(ui, |ui| {
                    let width = self.items.iter().map(|w| w.chars().count()).max().unwrap_or(8) as f32 * font.size * 0.62 + 24.0;
                    for (i, word) in self.items.iter().enumerate() {
                        let (rect, resp) = ui.allocate_exact_size(vec2(width.max(140.0), row_h), Sense::click());
                        if i == self.selected {
                            ui.painter().rect_filled(rect, 3.0, ui.visuals().selection.bg_fill);
                        }
                        ui.painter().text(rect.left_center() + vec2(8.0, 0.0), egui::Align2::LEFT_CENTER, word, font.clone(), ui.visuals().text_color());
                        if resp.clicked() {
                            chosen = Some(i);
                        }
                    }
                });
            });
        if let Some(i) = chosen {
            d.editor.complete(&self.items[i].clone());
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
        c.after_edit(&mut d, true);
        assert!(c.is_open());
        assert_eq!(c.items.len(), 2);
        c.selected = c.items.iter().position(|w| w == "alpine").unwrap();
        let word = c.items[c.selected].clone();
        d.editor.complete(&word);
        assert_eq!(d.editor.buffer.slice(0..d.editor.buffer.len_chars()), "alphabet alpine alpine");
    }

    #[test]
    fn explicit_with_one_match_completes_immediately() {
        let mut d = doc("unique un", 9);
        let mut c = Completion::default();
        c.open(&mut d, true);
        assert!(!c.is_open());
        assert_eq!(d.editor.buffer.slice(0..d.editor.buffer.len_chars()), "unique unique");
    }

    #[test]
    fn one_letter_does_not_pop_up() {
        let mut d = doc("apple a", 7);
        let mut c = Completion::default();
        c.after_edit(&mut d, true);
        assert!(!c.is_open());
    }
}
