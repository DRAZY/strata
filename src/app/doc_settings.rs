//! Each tab's settings: settings.json, the syntax's settings and
//! `.editorconfig` (see `crate::doc_settings`), then the indentation found in
//! the file and what was set for the tab by hand. Re-resolved whenever the
//! tab's syntax or path, the settings or an `.editorconfig` change.

use super::StrataApp;
use crate::buffer::{Buffer, LineEnding};
use crate::doc_settings::{Effective, Source, resolve, syntax_value, user_layer};
use crate::document::Document;
use crate::session::Settings;
use std::path::Path;

/// The window's part: parsed `.editorconfig` files, and which settings the
/// tabs were resolved against.
#[derive(Default)]
pub(super) struct State {
    pub(super) cache: crate::editorconfig::Cache,
    last: Option<Settings>,
    rev: u64,
    checked_at: Option<f64>,
}

/// The syntax whose settings a tab gets: a file in large-file mode shows as
/// Plain Text but keeps its own.
fn syntax_of(d: &Document) -> &str {
    d.large.as_ref().map_or_else(|| d.highlighter.syntax_name(), |l| l.syntax.as_str())
}

/// (spaces?, width when spaces showed one) found in the file's text.
fn detect(e: &mut crate::editor::Editor) -> Option<(bool, Option<usize>)> {
    // Width 0 marks "no width found" (tabs, or no steps between levels).
    let width = std::mem::replace(&mut e.tab_width, 0);
    let guess = e.guess_indentation();
    e.tab_width = width;
    guess.map(|(spaces, w)| (spaces, (w > 0).then_some(w)))
}

/// Does the text have line breaks other than `eol`?
fn has_other_endings(b: &Buffer, eol: LineEnding) -> bool {
    let mut prev = '\0';
    for chunk in b.chunks_in(0..b.len_chars()) {
        for c in chunk.chars() {
            if c == '\n' && ((prev == '\r') != (eol == LineEnding::CrLf)) {
                return true;
            }
            prev = c;
        }
    }
    false
}

impl StrataApp {
    /// Each frame: bring every tab's settings up to date (cheap when nothing
    /// changed), checking `.editorconfig` files for changes once a second.
    pub(super) fn refresh_doc_settings(&mut self, now: f64) {
        if self.doc_cfg.checked_at.is_none_or(|t| now - t >= 1.0 || now < t) {
            self.doc_cfg.checked_at = Some(now);
            self.doc_cfg.cache.check();
        }
        self.settings_rev();
        for i in 0..self.docs.len() {
            self.resolve_doc(i);
        }
    }

    /// Tab `i`'s settings, up to date.
    pub(super) fn doc_settings(&mut self, i: usize) -> &Effective {
        self.settings_rev();
        self.resolve_doc(i);
        &self.docs[i].local.effective
    }

    /// settings.json changed since the tabs were resolved: they're stale.
    fn settings_rev(&mut self) {
        if self.doc_cfg.last.as_ref() != Some(&self.settings) {
            self.doc_cfg.rev += 1;
            self.doc_cfg.last = Some(self.settings.clone());
        }
    }

    fn resolve_doc(&mut self, i: usize) {
        let (rev, ec_rev) = (self.doc_cfg.rev, self.doc_cfg.cache.rev);
        let d = &mut self.docs[i];
        let now = (d.editor.use_spaces, d.editor.tab_width);
        if d.local.applied_indent.is_some_and(|a| a != now) {
            // Changed from the status bar, View ▸ Indentation or Guess Settings From Buffer.
            d.local.tab_indent = Some(now);
        }
        let fresh = d.local.key.as_ref().is_some_and(|(r, e, syntax, path)| *r == rev && *e == ec_rev && syntax == syntax_of(d) && *path == d.path);
        if fresh && d.local.applied_indent == Some(now) {
            return;
        }
        let syntax = syntax_of(d).to_owned();
        let props = d.path.as_deref().filter(|_| self.settings.editorconfig).map(|p| self.doc_cfg.cache.properties(p));
        d.local.effective = resolve(&self.settings, &syntax, props.as_ref());
        if !d.local.detection_done {
            d.local.detection_done = true;
            if d.path.is_some() {
                d.local.detected = detect(&mut d.editor);
            }
        }
        let (spaces, width, from) = match d.local.tab_indent {
            Some((spaces, width)) => (spaces, width, Source::Tab),
            None => d.local.effective.indentation(d.local.detected),
        };
        (d.editor.use_spaces, d.editor.tab_width) = (spaces, width);
        d.local.applied_indent = Some((spaces, width));
        d.local.indent_from = from;
        // A new, empty file starts with .editorconfig's line ending and charset.
        if d.editor.buffer.len_chars() == 0 && !d.is_dirty() {
            if let Some(eol) = d.local.effective.end_of_line.filter(|_| !d.local.chose_line_ending) {
                d.editor.buffer.set_line_ending(eol);
            }
            if let Some(cs) = d.local.effective.charset {
                d.encoding = cs;
            }
        }
        d.local.key = Some((rev, ec_rev, syntax, d.path.clone()));
    }

    /// settings.json's indentation changed: tabs drop what was set for them
    /// by hand and take it (unless detection or `.editorconfig` say otherwise).
    pub(super) fn reset_tab_indentation(&mut self) {
        for d in &mut self.docs {
            d.local.tab_indent = None;
            d.local.key = None;
        }
        self.settings_rev();
        for i in 0..self.docs.len() {
            self.resolve_doc(i);
        }
    }

    /// Is word wrap on by default for tab `i` (before View ▸ Word Wrap)?
    pub(super) fn wraps_by_default(&self, i: usize) -> bool {
        let d = &self.docs[i];
        syntax_value(&self.settings, syntax_of(d), |l| l.word_wrap, self.settings.word_wrap) && d.large.is_none()
    }

    /// Is spell check on for tab `i`?
    pub(super) fn spell_on(&self, i: usize) -> bool {
        let d = &self.docs[i];
        d.local.spell_check.unwrap_or_else(|| syntax_value(&self.settings, syntax_of(d), |l| l.spell_check, self.settings.spell_check))
    }

    /// View ▸ Spell Check: flips it for the current tab, and makes that the
    /// setting (a tab whose syntax says otherwise keeps the choice for itself).
    pub(super) fn toggle_spell_check(&mut self) -> bool {
        let on = !self.spell_on(self.active);
        self.settings.spell_check = on;
        self.docs[self.active].local.spell_check = None;
        if self.spell_on(self.active) != on {
            self.docs[self.active].local.spell_check = Some(on);
        }
        on
    }

    /// Save-time changes for tab `i` about to be written to `path`, from its
    /// settings there: trim trailing whitespace, end with a newline, and
    /// `.editorconfig`'s line ending (unless one was picked for the tab) and,
    /// for a new file, charset.
    pub(super) fn save_time_settings(&mut self, i: usize, path: &Path) -> Effective {
        self.settings_rev();
        let d = &self.docs[i];
        let props = self.settings.editorconfig.then(|| self.doc_cfg.cache.properties(path));
        let eff = resolve(&self.settings, syntax_of(d), props.as_ref());
        let d = &mut self.docs[i];
        if let Some(eol) = eff.end_of_line.filter(|_| !d.local.chose_line_ending)
            && (d.editor.buffer.line_ending() != eol || has_other_endings(&d.editor.buffer, eol))
        {
            // Every cursor keeps its line and column.
            let b = &d.editor.buffer;
            let at = |p: usize| (b.line_of(p), p - b.line_start(b.line_of(p)));
            let sels: Vec<_> = d.editor.selections().iter().map(|s| (at(s.range().start), at(s.range().end))).collect();
            let chose = d.local.chose_line_ending;
            d.set_line_ending(eol);
            d.local.chose_line_ending = chose;
            let b = &d.editor.buffer;
            let back = |(line, col): (usize, usize)| {
                let line = line.min(b.len_lines() - 1);
                (b.line_start(line) + col).min(b.line_end(line))
            };
            let sels: Vec<_> = sels.into_iter().map(|(s, e)| back(s)..back(e)).collect();
            d.editor.select_ranges(&sels, 0);
        }
        let new_file = d.path.as_deref() != Some(path) && !path.exists();
        if let Some(cs) = eff.charset.filter(|_| new_file)
            && d.editor.buffer.check_encodable(cs).is_ok()
        {
            d.encoding = cs;
        }
        eff
    }

    /// After a View menu item changed `what` in settings.json: when the
    /// current file's own settings keep a different value, say who sets it.
    pub(super) fn note_override<T: PartialEq>(&mut self, what: &str, get: impl Fn(&Effective) -> T) {
        let i = self.active;
        let syntax = syntax_of(&self.docs[i]).to_owned();
        let has = get(self.doc_settings(i));
        let by_syntax = get(&resolve(&self.settings, &syntax, None));
        let from = if has != by_syntax {
            ".editorconfig".to_owned()
        } else if by_syntax != get(&Effective::from_settings(&self.settings)) {
            match user_layer(&self.settings, &syntax) {
                Some((key, _)) => format!("syntax_settings ({key})"),
                None => format!("Strata's default for {syntax}"),
            }
        } else {
            return;
        };
        self.status = format!("Saved, but {from} sets the {what} for {}", self.docs[i].name());
    }

    /// The status bar's indentation menu: where the current file's indentation came from.
    pub(super) fn indentation_source(&self) -> String {
        let local = &self.docs[self.active].local;
        let mut s = local.indent_from.describe();
        if local.indent_from == Source::EditorConfig
            && let Some(f) = local.effective.editorconfig.last()
        {
            s = format!("{s} ({})", f.display());
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;
    use crate::editor::Editor;

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("strata-docset-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn indent(app: &StrataApp) -> (bool, usize) {
        (app.docs[app.active].editor.use_spaces, app.docs[app.active].editor.tab_width)
    }

    #[test]
    fn syntax_settings_apply_and_a_syntax_switch_re_resolves() {
        let mut app = StrataApp::new(None, None);
        app.settings = Settings::parse(r#"{ "tab_size": 4, "rulers": [100], "syntax_settings": { "Python": { "tab_size": 3, "rulers": [79], "auto_indent": false } } }"#).unwrap();
        app.refresh_doc_settings(0.0);
        assert_eq!(indent(&app), (true, 4));
        app.doc().highlighter.set_syntax("Python");
        app.refresh_doc_settings(0.0);
        assert_eq!(indent(&app), (true, 3));
        assert_eq!(app.docs[0].local.indent_from, Source::Syntax("Python".into()));
        assert_eq!((app.docs[0].local.effective.rulers.clone(), app.docs[0].local.effective.auto_indent), (vec![79], false));
        app.doc().highlighter.set_syntax("YAML");
        app.refresh_doc_settings(0.0);
        assert_eq!(indent(&app), (true, 2), "Strata's YAML default");
        app.doc().highlighter.set_syntax("Go");
        app.refresh_doc_settings(0.0);
        assert_eq!(indent(&app), (false, 4));
        // A settings change reaches the tab.
        app.settings.syntax_settings.get_mut("Python").unwrap().tab_size = Some(6);
        app.doc().highlighter.set_syntax("Python");
        app.refresh_doc_settings(0.0);
        assert_eq!(indent(&app), (true, 6));
    }

    #[test]
    fn a_choice_made_for_the_tab_sticks() {
        let mut app = StrataApp::new(None, None);
        app.refresh_doc_settings(0.0);
        // As the status bar's menu does it.
        (app.doc().editor.use_spaces, app.doc().editor.tab_width) = (false, 8);
        app.refresh_doc_settings(0.0);
        assert_eq!(app.docs[0].local.indent_from, Source::Tab);
        app.doc().highlighter.set_syntax("YAML");
        app.refresh_doc_settings(0.0);
        assert_eq!(indent(&app), (false, 8), "a syntax change keeps it");
        app.settings.word_wrap = true;
        app.refresh_doc_settings(0.0);
        assert_eq!(indent(&app), (false, 8), "so does an unrelated settings change");
        // A new default indentation in settings.json replaces it.
        app.settings.tab_size = 5;
        app.apply_indentation();
        assert_eq!(indent(&app), (true, 2), "YAML's own");
    }

    #[test]
    fn editorconfig_beats_detection_which_beats_settings() {
        let dir = temp("ec");
        std::fs::write(dir.join("spaces2.py"), "def f():\n  return 1\n").unwrap();
        std::fs::write(dir.join("plain.rs"), "fn main() {\n  x();\n}\n").unwrap();
        std::fs::write(dir.join("Makefile"), "all:\n\tcc a.c\n").unwrap();
        std::fs::write(dir.join(".editorconfig"), "root = true\n[*.py]\nindent_style = tab\nindent_size = 8\n").unwrap();
        let mut app = StrataApp::new(None, None);
        app.open_path(dir.join("spaces2.py"), false);
        assert_eq!(indent(&app), (false, 8));
        assert_eq!(app.indentation_source(), format!("From .editorconfig ({})", dir.join(".editorconfig").display()));
        app.open_path(dir.join("plain.rs"), false);
        assert_eq!(indent(&app), (true, 2), "detected");
        assert_eq!(app.indentation_source(), "Detected from the file");
        app.open_path(dir.join("Makefile"), false);
        assert_eq!(indent(&app), (false, 4));
        // The editorconfig setting off: back to detection.
        app.settings.editorconfig = false;
        let py = app.tab_for_path(&dir.join("spaces2.py")).unwrap();
        app.activate(py);
        app.refresh_doc_settings(0.0);
        assert_eq!(indent(&app), (true, 2));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn an_editorconfig_change_is_picked_up() {
        let dir = temp("change");
        std::fs::write(dir.join("a.txt"), "").unwrap();
        let mut app = StrataApp::new(None, None);
        app.open_path(dir.join("a.txt"), false);
        assert_eq!(indent(&app), (true, 4));
        std::fs::write(dir.join(".editorconfig"), "[*]\nindent_size = 2\n").unwrap();
        app.refresh_doc_settings(10.0);
        assert_eq!(indent(&app), (true, 2), "a new .editorconfig, noticed within a second");
        std::fs::write(dir.join(".editorconfig"), "[*]\nindent_size = 3\n").unwrap();
        app.doc_cfg.cache.saved(&dir.join(".editorconfig"));
        app.refresh_doc_settings(10.1);
        assert_eq!(indent(&app), (true, 3), "saved in Strata: at once");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn editorconfig_charset_and_line_ending_for_new_files() {
        let dir = temp("new");
        std::fs::write(dir.join(".editorconfig"), "[*.txt]\nend_of_line = crlf\ncharset = latin1\n").unwrap();
        let mut app = StrataApp::new(None, None);
        app.open_path(dir.join("new.txt"), false);
        let d = &app.docs[app.active];
        assert_eq!((d.editor.buffer.line_ending(), d.encoding), (LineEnding::CrLf, crate::encoding::Encoding::ISO_8859_1));
        app.doc().editor.insert_text("caf\u{e9}\nau lait\n");
        let path = dir.join("new.txt");
        assert!(app.save(app.active, false));
        assert_eq!(std::fs::read(&path).unwrap(), b"caf\xe9\r\nau lait\r\n");
        // An untitled tab saved there for the first time takes the charset too.
        app.push_doc(Document::untitled());
        app.doc().editor.insert_text("na\u{ef}ve\n");
        app.save_time_settings(app.active, &dir.join("other.txt"));
        assert_eq!(app.docs[app.active].encoding, crate::encoding::Encoding::ISO_8859_1);
        assert_eq!(app.docs[app.active].editor.buffer.slice(0..app.docs[app.active].editor.buffer.len_chars()), "na\u{ef}ve\r\n");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn editorconfig_converts_line_endings_and_trims_on_save() {
        let dir = temp("save");
        std::fs::write(dir.join(".editorconfig"), "[*]\nend_of_line = lf\ntrim_trailing_whitespace = true\ninsert_final_newline = true\n").unwrap();
        let path = dir.join("a.txt");
        std::fs::write(&path, "one  \r\ntwo\t\r\nthree").unwrap();
        let mut app = StrataApp::new(None, None);
        app.open_path(path.clone(), false);
        app.doc().editor.select_ranges(&[0..0, 7..7], 0);
        app.doc().editor.insert_text("x");
        assert!(app.save(app.active, false));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "xone\nxtwo\nthree\n");
        let e = &app.docs[app.active].editor;
        let lines: Vec<usize> = e.selections().iter().map(|s| e.buffer.line_of(s.head)).collect();
        assert_eq!(lines, [0, 1], "cursors keep their lines");
        // A line ending picked in the status bar is left alone.
        app.docs[app.active].set_line_ending(LineEnding::CrLf);
        assert!(app.save(app.active, false));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "xone\r\nxtwo\r\nthree\r\n");
        // insert_final_newline = false wins over the setting.
        std::fs::write(dir.join(".editorconfig"), "[*]\ninsert_final_newline = false\ntrim_trailing_whitespace = false\n").unwrap();
        app.doc_cfg.cache.saved(&dir.join(".editorconfig"));
        app.settings.ensure_newline_at_eof_on_save = true;
        let b = dir.join("b.txt");
        std::fs::write(&b, "end  ").unwrap();
        app.open_path(b.clone(), false);
        app.doc().editor.insert_text("x");
        assert!(app.save(app.active, false));
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "xend  ");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn has_other_endings_finds_mixed_text() {
        assert!(!has_other_endings(&Buffer::from_str("a\nb\n"), LineEnding::Lf));
        assert!(has_other_endings(&Buffer::from_str("a\r\nb\n"), LineEnding::Lf));
        assert!(has_other_endings(&Buffer::from_str("a\r\nb\n"), LineEnding::CrLf));
        assert!(!has_other_endings(&Buffer::from_str("a\r\nb\r\n"), LineEnding::CrLf));
    }

    #[test]
    fn markdown_wraps_and_syntax_settings_can_say_otherwise() {
        let mut app = StrataApp::new(None, None);
        app.doc().editor = Editor::new(Buffer::from_str("text"));
        app.doc().highlighter.set_syntax("Markdown");
        assert!(app.wraps(0));
        app.settings.syntax_settings.insert("markdown".into(), crate::doc_settings::Layer { word_wrap: Some(false), spell_check: Some(true), ..Default::default() });
        assert!(!app.wraps(0) && app.spell_on(0));
        app.doc().highlighter.set_syntax("Rust");
        assert!(!app.spell_on(0));
        // The menu turns it on for this tab (and makes it the setting).
        assert!(app.toggle_spell_check());
        assert!(app.settings.spell_check && app.docs[0].local.spell_check.is_none());
        app.doc().highlighter.set_syntax("Markdown");
        assert!(!app.toggle_spell_check() && !app.spell_on(0));
        assert_eq!(app.docs[0].local.spell_check, Some(false), "Markdown's own setting would keep it on");
    }
}
