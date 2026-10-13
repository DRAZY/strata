//! Spell check in the window: keeping each visible editor's underlines up
//! to date, View ▸ Spell Check / Next and Previous Misspelling / Dictionary,
//! and the suggestions at the top of the editor's right-click menu.

use super::StrataApp;
use crate::commands::Cmd;
use crate::menus::{Act, Item};
use crate::spell::{self, Speller};
use std::cell::RefCell;
use std::ops::Range;

thread_local! {
    /// One checker for every window (they all run on the UI thread).
    static SPELLER: RefCell<Speller> = RefCell::new(Speller::default());
    /// The last dictionary or selector problem shown, so it's said once.
    static REPORTED: RefCell<Option<String>> = const { RefCell::new(None) };
}

// Menu actions (`Act::Named`).
const FIX: &str = "spell:fix:";
const IGNORE: &str = "spell:ignore:";
const ADD: &str = "spell:add:";
const DICTIONARY: &str = "spell:dict:";

/// Lines checked per step of Next / Previous Misspelling.
const BLOCK: usize = 2_000;

impl StrataApp {
    fn with_speller<R>(&self, f: impl FnOnce(&mut Speller) -> R) -> R {
        SPELLER.with(|sp| {
            let mut sp = sp.borrow_mut();
            let s = &self.settings;
            sp.configure(&s.dictionary, &s.added_words, &s.spelling_selector);
            f(&mut sp)
        })
    }

    fn report_once(&mut self, problem: Option<String>) {
        let new = REPORTED.with(|r| {
            let mut r = r.borrow_mut();
            let new = problem.is_some() && *r != problem;
            r.clone_from(&problem);
            new
        });
        if new && let Some(p) = problem {
            self.status = format!("Spell check: {p}");
        }
    }

    /// Each frame: underline misspellings in the lines each pane shows.
    pub(super) fn update_spelling(&mut self, ctx: &egui::Context) {
        if !self.settings.spell_check {
            for d in &mut self.docs {
                d.highlighter.set_prose_selector(None);
                d.view.spell.clear();
            }
            return;
        }
        let (selector, dict, problem) = self.with_speller(|sp| {
            let dict = spell::dictionary(sp.dictionary_name(), Some(ctx));
            let problem = sp.error.clone().or_else(|| dict.as_ref().err().cloned());
            (sp.selector(), dict.ok().flatten(), problem)
        });
        self.report_once(problem);
        let shown: Vec<usize> = (0..self.layout.groups()).filter_map(|g| self.group_active(g)).collect();
        for i in shown {
            let d = &mut self.docs[i];
            d.highlighter.set_prose_selector(selector.clone());
            let Some(dict) = &dict else { continue };
            let visible = d.view.visible_lines();
            let changed = SPELLER.with(|sp| d.view.spell.refresh(&d.editor.buffer, &d.highlighter, visible, &mut sp.borrow_mut(), dict));
            if changed {
                ctx.request_repaint();
            }
        }
    }

    pub(super) fn run_spelling(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::ToggleSpellCheck => {
                self.settings.spell_check = !self.settings.spell_check;
                self.persist_settings();
                self.status = format!("Spell check {}", if self.settings.spell_check { "on" } else { "off" });
            }
            Cmd::NextMisspelling => self.step_misspelling(true),
            Cmd::PrevMisspelling => self.step_misspelling(false),
            _ => {}
        }
    }

    /// Misspelled words (absolute char ranges) in `lines` of the current tab.
    fn misspellings_in(&mut self, lines: Range<usize>, dict: &spellbook::Dictionary) -> Vec<Range<usize>> {
        let selector = self.with_speller(|sp| sp.selector());
        let d = &mut self.docs[self.active];
        d.highlighter.set_prose_selector(selector);
        let mut out = Vec::new();
        let (hl, buf) = (&d.highlighter, &d.editor.buffer);
        SPELLER.with(|sp| {
            let mut sp = sp.borrow_mut();
            hl.prose_lines(buf, lines.start, |line, text, prose| {
                if line >= lines.end {
                    return false;
                }
                let start = buf.line_start(line);
                out.extend(sp.misspelled(dict, text, prose).into_iter().map(|r| start + r.start..start + r.end));
                true
            });
        });
        out
    }

    /// View ▸ Next / Prev Misspelling: select the next misspelled word
    /// after the selection (or before it), wrapping around the file.
    fn step_misspelling(&mut self, forward: bool) {
        if !self.settings.spell_check {
            self.status = format!("Spell check is off ({} turns it on)", Cmd::ToggleSpellCheck.shortcut_label().unwrap_or_else(|| "View ▸ Spell Check".into()));
            return;
        }
        let name = self.settings.dictionary.clone();
        let dict = match spell::dictionary_now(&name) {
            Ok(d) => d,
            Err(e) => {
                self.status = format!("Spell check: {e}");
                return;
            }
        };
        let ed = &self.docs[self.active].editor;
        let (sel, n) = (ed.primary().range(), ed.buffer.len_lines());
        let here = ed.buffer.line_of(if forward { sel.end } else { sel.start });
        let found = if forward {
            // From the cursor's line to the end, then from the top.
            let mut found = None;
            let mut from = here;
            while found.is_none() && from < n {
                found = self.misspellings_in(from..(from + BLOCK).min(n), &dict).into_iter().find(|r| r.start >= sel.end);
                from += BLOCK;
            }
            found.or_else(|| self.misspellings_in(0..here + 1, &dict).into_iter().next())
        } else {
            // Back in blocks from the cursor's line, then up from the end.
            let mut found = None;
            let mut to = here + 1;
            while found.is_none() && to > 0 {
                let from = to.saturating_sub(BLOCK);
                found = self.misspellings_in(from..to, &dict).into_iter().rfind(|r| r.end <= sel.start);
                to = from;
            }
            found.or_else(|| self.misspellings_in(here..n, &dict).pop())
        };
        match found {
            Some(r) => {
                let d = self.doc();
                d.editor.select_ranges(&[r], 0);
                d.view.reveal_centered();
            }
            None => self.status = "No misspellings".into(),
        }
    }

    /// The misspelled word under tab `i`'s cursor: its char range and text.
    fn misspelling_at_cursor(&self, i: usize) -> Option<(Range<usize>, String)> {
        let d = &self.docs[i];
        let head = d.editor.primary().head;
        let line = d.editor.buffer.line_of(head);
        let start = d.editor.buffer.line_start(line);
        let text = d.editor.buffer.line_text(line);
        let prose = d.highlighter.prose(line)?;
        let dict = spell::dictionary(&self.settings.dictionary, None).ok().flatten()?;
        let found = self.with_speller(|sp| sp.misspelled(&dict, &text, &prose));
        let r = found.into_iter().find(|r| start + r.start <= head && head <= start + r.end)?;
        let word = text.chars().skip(r.start).take(r.len()).collect();
        Some((start + r.start..start + r.end, word))
    }

    /// Suggestions, Ignore Word and Add Word for a misspelled word under
    /// the cursor (empty otherwise): the top of the editor's context menu.
    pub(super) fn spelling_items(&self, i: usize) -> Vec<Item> {
        if !self.settings.spell_check || i != self.active || self.docs[i].editor.read_only {
            return Vec::new();
        }
        let Some((range, word)) = self.misspelling_at_cursor(i) else { return Vec::new() };
        let Ok(Some(dict)) = spell::dictionary(&self.settings.dictionary, None) else { return Vec::new() };
        let suggestions = self.with_speller(|sp| sp.suggest(&dict, &word));
        let mut items: Vec<Item> = suggestions.into_iter().map(|s| Item::new(s.clone(), Act::Named(format!("{FIX}{}:{}:{s}", range.start, range.end)))).collect();
        if items.is_empty() {
            items.push(Item::new("No Suggestions", Act::Named(String::new())).enabled(false));
        }
        items.push(Item::Separator);
        items.push(Item::new("Ignore Word", Act::Named(format!("{IGNORE}{word}"))));
        items.push(Item::new("Add Word to Dictionary", Act::Named(format!("{ADD}{word}"))));
        items.push(Item::Separator);
        items
    }

    /// View ▸ Spell Check, Next / Previous Misspelling and Dictionary.
    pub(super) fn spelling_menu(&self) -> Vec<Item> {
        let dictionaries = spell::DICTIONARIES
            .iter()
            .map(|&(name, label)| Item::new(format!("{name} — {label}"), Act::Named(format!("{DICTIONARY}{name}"))).checked(self.settings.dictionary == name))
            .collect();
        vec![
            Item::cmd("Spell Check", Cmd::ToggleSpellCheck).checked(self.settings.spell_check),
            Item::cmd("Next Misspelling", Cmd::NextMisspelling).enabled(self.settings.spell_check),
            Item::cmd("Prev Misspelling", Cmd::PrevMisspelling).enabled(self.settings.spell_check),
            Item::Sub("Dictionary".into(), dictionaries),
        ]
    }

    /// A spelling menu action (`spell:…`).
    pub(super) fn spell_act(&mut self, act: &str) {
        if let Some(rest) = act.strip_prefix(FIX) {
            let mut parts = rest.splitn(3, ':');
            let (Some(a), Some(b), Some(with)) = (parts.next(), parts.next(), parts.next()) else { return };
            let (Ok(a), Ok(b)) = (a.parse::<usize>(), b.parse::<usize>()) else { return };
            let d = self.doc();
            if a <= b && b <= d.editor.buffer.len_chars() {
                d.editor.replace_ranges(vec![(a..b, with.to_owned())]);
                d.view.focus();
            }
        } else if let Some(word) = act.strip_prefix(IGNORE) {
            self.with_speller(|sp| sp.ignore(word));
            self.status = format!("Ignoring “{word}” until Strata quits");
        } else if let Some(word) = act.strip_prefix(ADD) {
            if !self.settings.added_words.iter().any(|w| w == word) {
                self.settings.added_words.push(word.to_owned());
                self.persist_settings();
            }
            self.status = format!("Added “{word}” to added_words in settings.json");
        } else if let Some(name) = act.strip_prefix(DICTIONARY) {
            self.settings.dictionary = name.to_owned();
            self.persist_settings();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;
    use crate::editor::Editor;
    use crate::menus::Item;
    use egui::{Context, RawInput, Rect, pos2, vec2};

    fn app_with(path: &str, text: &str) -> StrataApp {
        let mut app = StrataApp::new(None, None);
        let d = &mut app.docs[0];
        d.editor = Editor::new(Buffer::from_str(text));
        d.highlighter.detect(Some(std::path::Path::new(path)), "");
        app.settings.spell_check = true;
        crate::spell::dictionary_now("en_US").unwrap();
        app
    }

    /// Draw a frame (the editor paints, then spelling catches up).
    fn frame(app: &mut StrataApp, ctx: &Context) {
        for _ in 0..3 {
            let raw = RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 700.0))), ..Default::default() };
            let mut out = ctx.run_ui(raw, |ui| {
                let d = &mut app.docs[0];
                d.view.show(ui, &mut d.editor, &mut d.highlighter, &[]);
            });
            out.textures_delta.clear();
            app.update_spelling(ctx);
        }
    }

    /// Wavy underlines painted in one more frame: each one's (left, right).
    fn squiggles(app: &mut StrataApp, ctx: &Context) -> Vec<(f32, f32)> {
        let raw = RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 700.0))), ..Default::default() };
        let mut out = ctx.run_ui(raw, |ui| {
            let d = &mut app.docs[0];
            d.view.show(ui, &mut d.editor, &mut d.highlighter, &[]);
        });
        out.textures_delta.clear();
        out.shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Path(p) if p.points.len() > 2 && !p.closed && p.points.windows(2).all(|w| (w[0].y - w[1].y).abs() == 2.0) => {
                    Some((p.points[0].x, p.points.last().unwrap().x))
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_editor_paints_a_wavy_line_under_each_misspelling() {
        let mut app = app_with("notes.txt", "one wrod and anothr\nfine\n");
        let ctx = Context::default();
        frame(&mut app, &ctx);
        let lines = squiggles(&mut app, &ctx);
        assert_eq!(lines.len(), 2, "{lines:?}");
        // "wrod" is 4 characters, "anothr" 6: the second line is 1.5 times as long.
        let (w0, w1) = ((lines[0].1 - lines[0].0), (lines[1].1 - lines[1].0));
        assert!((w1 / w0 - 1.5).abs() < 0.05, "{w0} {w1}");
        assert!(lines[0].1 < lines[1].0);
        app.settings.spell_check = false;
        frame(&mut app, &ctx);
        assert!(squiggles(&mut app, &ctx).is_empty());
    }

    fn labels(items: &[Item]) -> Vec<String> {
        items
            .iter()
            .map(|it| match it {
                Item::Action { label, .. } => label.clone(),
                Item::Sub(label, _) => label.clone(),
                _ => "---".into(),
            })
            .collect()
    }

    #[test]
    fn misspellings_are_underlined_in_comments_only() {
        let mut app = app_with("a.rs", "// a wrod in a comment\nlet wrod = \"another wrod\";\n");
        let ctx = Context::default();
        frame(&mut app, &ctx);
        let d = &app.docs[0];
        let line = |l: usize| d.editor.buffer.line_text(l);
        assert_eq!(d.view.spell.for_line(0, &line(0)), Some(&[5..9][..]));
        assert_eq!(d.view.spell.for_line(1, &line(1)), Some(&[20..24][..]), "the string, not the variable");
        app.settings.spell_check = false;
        app.update_spelling(&ctx);
        assert!(app.docs[0].view.spell.is_empty(), "off clears the underlines");
    }

    #[test]
    fn context_menu_offers_suggestions_ignore_and_add() {
        let mut app = app_with("notes.txt", "a speling mistake\n");
        let ctx = Context::default();
        frame(&mut app, &ctx);
        app.docs[0].editor.select_ranges(&[4..4], 0);
        let items = app.spelling_items(0);
        let names = labels(&items);
        assert!(names.contains(&"spelling".to_owned()), "{names:?}");
        assert!(names.len() <= crate::spell::MAX_SUGGESTIONS + 4);
        assert!(names.ends_with(&["---".into(), "Ignore Word".into(), "Add Word to Dictionary".into(), "---".into()]));
        // Picking a suggestion replaces the word.
        let fix = items.iter().find_map(|it| match it {
            Item::Action { label, act: Act::Named(n), .. } if label == "spelling" => Some(n.clone()),
            _ => None,
        });
        app.spell_act(&fix.unwrap());
        assert_eq!(app.docs[0].editor.buffer.line_text(0), "a spelling mistake");
        // A correct word gets no spelling items.
        app.docs[0].editor.select_ranges(&[1..1], 0);
        frame(&mut app, &ctx);
        assert!(app.spelling_items(0).is_empty());
    }

    #[test]
    fn add_word_persists_to_added_words_and_ignore_lasts_the_session() {
        let mut app = app_with("notes.txt", "Zorblax is an editr\n");
        let ctx = Context::default();
        frame(&mut app, &ctx);
        let d = &app.docs[0];
        assert_eq!(d.view.spell.for_line(0, &d.editor.buffer.line_text(0)), Some(&[0..7, 14..19][..]));
        app.spell_act("spell:add:Zorblax");
        assert_eq!(app.settings.added_words, ["Zorblax"]);
        app.spell_act("spell:add:Zorblax");
        assert_eq!(app.settings.added_words.len(), 1, "added once");
        app.spell_act("spell:ignore:editr");
        frame(&mut app, &ctx);
        let d = &app.docs[0];
        assert_eq!(d.view.spell.for_line(0, &d.editor.buffer.line_text(0)), None);
        // Ignored words are this run's only: forget them for the other tests.
        SPELLER.with(|sp| *sp.borrow_mut() = Speller::default());
    }

    #[test]
    fn next_and_previous_misspelling_wrap() {
        let mut text = String::from("# Notes\n\nfirst wrod here\n");
        for _ in 0..3000 {
            text.push_str("all fine words\n");
        }
        text.push_str("last speling\n");
        let mut app = app_with("notes.md", &text);
        app.docs[0].editor.select_ranges(&[0..0], 0);
        let sel = |app: &StrataApp| app.docs[0].editor.buffer.slice(app.docs[0].editor.primary().range());
        app.run_spelling(Cmd::NextMisspelling);
        assert_eq!(sel(&app), "wrod");
        app.run_spelling(Cmd::NextMisspelling);
        assert_eq!(sel(&app), "speling", "past the first block of lines");
        app.run_spelling(Cmd::NextMisspelling);
        assert_eq!(sel(&app), "wrod", "wraps to the top");
        app.run_spelling(Cmd::PrevMisspelling);
        assert_eq!(sel(&app), "speling", "wraps to the bottom");
        app.run_spelling(Cmd::PrevMisspelling);
        assert_eq!(sel(&app), "wrod");
    }

    #[test]
    fn toggling_dictionary_and_menu() {
        let mut app = app_with("notes.txt", "colour\n");
        app.settings.spell_check = false;
        app.run_spelling(Cmd::NextMisspelling);
        assert!(app.status.starts_with("Spell check is off"), "{}", app.status);
        app.run_spelling(Cmd::ToggleSpellCheck);
        assert!(app.settings.spell_check);
        let menu = app.spelling_menu();
        assert_eq!(labels(&menu), ["Spell Check", "Next Misspelling", "Prev Misspelling", "Dictionary"]);
        let ctx = Context::default();
        frame(&mut app, &ctx);
        assert!(!app.docs[0].view.spell.is_empty(), "colour is British");
        app.spell_act("spell:dict:en_GB");
        assert_eq!(app.settings.dictionary, "en_GB");
        crate::spell::dictionary_now("en_GB").unwrap();
        frame(&mut app, &ctx);
        assert!(app.docs[0].view.spell.is_empty());
    }

    #[test]
    fn spelling_keys_follow_sublime() {
        use crate::keys::*;
        assert_eq!(Cmd::ToggleSpellCheck.keys(), vec![(NONE, egui::Key::F6)]);
        assert_eq!(Cmd::NextMisspelling.keys(), vec![(CTRL, egui::Key::F6)]);
        assert_eq!(Cmd::PrevMisspelling.keys(), vec![(CTRL_SHIFT, egui::Key::F6)]);
    }
}
