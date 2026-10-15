//! What was searched for: ↑ and ↓ in the find bar's fields and in Find in
//! Files' recall earlier entries, newest first. One history per field, shared
//! by every window and kept in the session.

use egui::text::{CCursor, CCursorRange};
use egui::{Id, Key, Modifiers, Ui};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// Entries kept per field.
pub const LIMIT: usize = 50;

/// A field with a history.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Find,
    Replace,
    FilesFind,
    FilesWhere,
    FilesReplace,
}

/// Each field's entries, newest first, without repeats.
#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct FindHistory {
    pub find: Vec<String>,
    pub replace: Vec<String>,
    pub files_find: Vec<String>,
    pub files_where: Vec<String>,
    pub files_replace: Vec<String>,
}

impl FindHistory {
    pub fn is_empty(&self) -> bool {
        self.find.is_empty() && self.replace.is_empty() && self.files_find.is_empty() && self.files_where.is_empty() && self.files_replace.is_empty()
    }

    pub fn entries(&self, field: Field) -> &[String] {
        match field {
            Field::Find => &self.find,
            Field::Replace => &self.replace,
            Field::FilesFind => &self.files_find,
            Field::FilesWhere => &self.files_where,
            Field::FilesReplace => &self.files_replace,
        }
    }

    fn entries_mut(&mut self, field: Field) -> &mut Vec<String> {
        match field {
            Field::Find => &mut self.find,
            Field::Replace => &mut self.replace,
            Field::FilesFind => &mut self.files_find,
            Field::FilesWhere => &mut self.files_where,
            Field::FilesReplace => &mut self.files_replace,
        }
    }

    /// Put `text` first (moving it up if it's there already). Empty text
    /// isn't kept, except in Replace fields where "replace with nothing" is
    /// a real choice. False if nothing changed.
    pub fn add(&mut self, field: Field, text: &str) -> bool {
        let blank_ok = matches!(field, Field::Replace | Field::FilesReplace);
        if (text.is_empty() && !blank_ok) || text.len() > 10_000 {
            return false;
        }
        let list = self.entries_mut(field);
        if list.first().is_some_and(|t| t == text) {
            return false;
        }
        list.retain(|t| t != text);
        list.insert(0, text.to_owned());
        list.truncate(LIMIT);
        true
    }

    /// Tidy a history read from disk: no repeats, at most `LIMIT` each.
    fn sanitized(mut self) -> Self {
        for field in [Field::Find, Field::Replace, Field::FilesFind, Field::FilesWhere, Field::FilesReplace] {
            let list = self.entries_mut(field);
            let mut seen = std::collections::HashSet::new();
            list.retain(|t| seen.insert(t.clone()));
            list.truncate(LIMIT);
        }
        self
    }
}

/// The history every window shares, and a count of its changes.
static HISTORY: Mutex<(FindHistory, u64)> = Mutex::new((
    FindHistory { find: Vec::new(), replace: Vec::new(), files_find: Vec::new(), files_where: Vec::new(), files_replace: Vec::new() },
    0,
));

fn lock() -> std::sync::MutexGuard<'static, (FindHistory, u64)> {
    HISTORY.lock().unwrap_or_else(|e| e.into_inner())
}

/// Remember a search (or replacement) that was run.
pub fn record(field: Field, text: &str) {
    let mut h = lock();
    if h.0.add(field, text) {
        h.1 += 1;
    }
}

/// The history, to save with the session.
pub fn snapshot() -> FindHistory {
    lock().0.clone()
}

/// Bumped by every change (the session is saved when it changes).
pub fn generation() -> u64 {
    lock().1
}

/// Start from the history a session saved.
pub fn restore(saved: &FindHistory) {
    let mut h = lock();
    h.0 = saved.clone().sanitized();
    h.1 += 1;
}

/// Where ↑/↓ browsing in one field is: the entry shown (None: the user's
/// own text, kept in `draft`).
#[derive(Clone, Default)]
struct Browse {
    at: Option<usize>,
    draft: String,
    shown: String,
}

/// Step through `entries` from `b`: ↑ (`older`) goes back in time, ↓ forward
/// and finally back to the draft. The new text, or None at either end.
fn step(entries: &[String], b: &mut Browse, current: &str, older: bool) -> Option<String> {
    // Typing after recalling an entry starts a new draft.
    if b.at.is_some() && current != b.shown {
        b.at = None;
    }
    let mut next = match (b.at, older) {
        (None, true) => (!entries.is_empty()).then_some(0)?,
        (None, false) => return None,
        (Some(i), true) => (i + 1 < entries.len()).then_some(i + 1)?,
        (Some(0), false) => {
            b.at = None;
            b.shown.clear();
            return Some(std::mem::take(&mut b.draft));
        }
        (Some(i), false) => i - 1,
    };
    // Going back, skip an entry equal to what's in the field already.
    if older && entries[next] == current && next + 1 < entries.len() {
        next += 1;
    }
    if b.at.is_none() {
        b.draft = current.to_owned();
    }
    b.at = Some(next);
    b.shown.clone_from(&entries[next]);
    Some(entries[next].clone())
}

/// Call just before drawing the text field `id`: while it has focus, ↑ and
/// ↓ (no modifiers) replace its text with earlier entries of `field`, caret
/// at the end.
pub fn keys(ui: &mut Ui, id: Id, field: Field, text: &mut String) {
    if !ui.memory(|m| m.has_focus(id)) {
        return;
    }
    let older = if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowUp)) {
        true
    } else if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowDown)) {
        false
    } else {
        return;
    };
    let ctx = ui.ctx().clone();
    let key = id.with("find-history");
    let mut browse: Browse = ctx.data(|d| d.get_temp(key)).unwrap_or_default();
    let entries = lock().0.entries(field).to_vec();
    if let Some(new) = step(&entries, &mut browse, text, older) {
        *text = new;
        let mut state = egui::TextEdit::load_state(&ctx, id).unwrap_or_default();
        state.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(text.chars().count()))));
        egui::TextEdit::store_state(&ctx, id, state);
    }
    ctx.data_mut(|d| d.insert_temp(key, browse));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn newest_first_without_repeats_and_capped() {
        let mut h = FindHistory::default();
        assert!(h.add(Field::Find, "a"));
        assert!(h.add(Field::Find, "b"));
        assert!(h.add(Field::Find, "a"));
        assert!(!h.add(Field::Find, "a"), "already first");
        assert!(!h.add(Field::Find, ""), "empty searches aren't kept");
        assert_eq!(h.find, history(&["a", "b"]));
        assert!(h.add(Field::Replace, ""), "replacing with nothing is a choice");
        for i in 0..80 {
            h.add(Field::FilesFind, &format!("q{i}"));
        }
        assert_eq!(h.files_find.len(), LIMIT);
        assert_eq!(h.files_find[0], "q79");
        assert!(h.files_where.is_empty() && h.replace == [""], "fields are separate");
    }

    #[test]
    fn up_goes_back_down_returns_to_the_draft() {
        let entries = history(&["newest", "middle", "oldest"]);
        let mut b = Browse::default();
        let mut text = "dra".to_owned();
        let mut press = |text: &mut String, older: bool| {
            if let Some(t) = step(&entries, &mut b, text, older) {
                *text = t;
            }
        };
        press(&mut text, false);
        assert_eq!(text, "dra", "↓ with nothing recalled does nothing");
        press(&mut text, true);
        assert_eq!(text, "newest");
        press(&mut text, true);
        press(&mut text, true);
        assert_eq!(text, "oldest");
        press(&mut text, true);
        assert_eq!(text, "oldest", "stops at the oldest");
        press(&mut text, false);
        assert_eq!(text, "middle");
        press(&mut text, false);
        press(&mut text, false);
        assert_eq!(text, "dra", "back to what was typed");
        // Editing a recalled entry makes it the new draft.
        press(&mut text, true);
        text.push('!');
        press(&mut text, true);
        assert_eq!(text, "newest");
        press(&mut text, false);
        assert_eq!(text, "newest!");
    }

    #[test]
    fn the_entry_already_shown_is_skipped() {
        let entries = history(&["same", "older"]);
        let mut b = Browse::default();
        assert_eq!(step(&entries, &mut b, "same", true).as_deref(), Some("older"));
    }

    #[test]
    fn sessions_without_a_history_still_load() {
        let s: crate::session::Session = serde_json::from_str(r#"{"tabs": []}"#).unwrap();
        assert!(s.find_history.is_empty());
        let mut s = crate::session::Session::default();
        s.find_history.add(Field::FilesWhere, "*.rs");
        let back: crate::session::Session = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.find_history.files_where, ["*.rs"]);
        let repeated: FindHistory = serde_json::from_str(r#"{"find": ["a", "a", "b"]}"#).unwrap();
        assert_eq!(repeated.sanitized().find, ["a", "b"]);
    }

    #[test]
    fn arrows_in_a_focused_field_recall_entries() {
        let ctx = egui::Context::default();
        let id = Id::new("test-find-history-field");
        let mut text = String::new();
        let frame = |text: &mut String, events: Vec<egui::Event>| {
            let raw = egui::RawInput { events, ..Default::default() };
            let mut out = ctx.run_ui(raw, |ui| {
                ui.memory_mut(|m| m.request_focus(id));
                // A field of its own: the shared history may hold other tests' entries.
                let saved = snapshot();
                restore(&FindHistory { files_replace: history(&["two", "one"]), ..Default::default() });
                keys(ui, id, Field::FilesReplace, text);
                restore(&saved);
                ui.add(egui::TextEdit::singleline(text).id(id));
            });
            out.textures_delta.clear();
        };
        let up = egui::Event::Key { key: Key::ArrowUp, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE };
        frame(&mut text, vec![]);
        frame(&mut text, vec![up.clone()]);
        assert_eq!(text, "two");
        frame(&mut text, vec![up]);
        assert_eq!(text, "one");
        let caret = egui::TextEdit::load_state(&ctx, id).and_then(|s| s.cursor.char_range()).map(|r| r.primary.index.0);
        assert_eq!(caret, Some(3), "the caret goes to the end");
    }
}
