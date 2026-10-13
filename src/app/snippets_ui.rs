//! Snippets in the app: Tab expands a trigger, Tab / Shift+Tab walk the
//! fields, Esc stops; Tools ▸ Snippets… lists the ones valid at the caret and
//! Tools ▸ Developer ▸ New Snippet… starts a new `.sublime-snippet`.

use super::StrataApp;
use super::more::Choosing;
use crate::buffer::Buffer;
use crate::document::Document;
use crate::snippets::{self, Library, Session, Snippet};
use egui::{Event, Key, Modifiers};

const HINT: &str = "Snippet field";

/// Seconds between checks of the Snippets folder for changes.
const RESCAN: f64 = 2.0;

const TEMPLATE: &str = r#"<snippet>
	<content><![CDATA[
Hello, ${1:this} is a ${2:snippet}.$0
]]></content>
	<!-- Optional: type this, then Tab, to insert the snippet -->
	<!-- <tabTrigger>hello</tabTrigger> -->
	<!-- Optional: where it applies (scope selector), e.g. source.python -->
	<!-- <scope>source.python</scope> -->
	<!-- Optional: shown in Tools ▸ Snippets… and the completion list -->
	<!-- <description>Hello</description> -->
</snippet>
"#;

#[derive(Default)]
pub(super) struct SnippetState {
    pub lib: Library,
    /// Fields being edited: (editor view id, session).
    session: Option<(egui::Id, Session)>,
    last_scan: f64,
    /// Tabs made by New Snippet…, which save into the Snippets folder.
    template_tabs: Vec<egui::Id>,
}

impl SnippetState {
    pub fn new(config: Option<&std::path::Path>) -> Self {
        Self { lib: Library::load(config), ..Default::default() }
    }
}

/// Take the first key press of `key` with exactly `mods`.
fn take_key(ctx: &egui::Context, key: Key, mods: Modifiers) -> bool {
    ctx.input_mut(|i| {
        let pos = i.events.iter().position(|e| matches!(e, Event::Key { key: k, pressed: true, modifiers, .. } if *k == key && *modifiers == mods));
        pos.map(|p| i.events.remove(p)).is_some()
    })
}

fn has_key(ctx: &egui::Context, key: Key, mods: Modifiers) -> bool {
    ctx.input(|i| i.events.iter().any(|e| matches!(e, Event::Key { key: k, pressed: true, modifiers, .. } if *k == key && *modifiers == mods)))
}

impl StrataApp {
    /// Each frame, before the editor handles keys: reload changed snippet
    /// files, insert a snippet chosen in the completion list, and take Tab,
    /// Shift+Tab and Esc for triggers and fields.
    pub(super) fn snippet_keys(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        if now - self.snippets.last_scan > RESCAN || now < self.snippets.last_scan {
            self.snippets.last_scan = now;
            if self.snippets.lib.refresh() {
                self.report_snippet_errors();
            }
        }
        self.snippet_from_completion();
        self.check_snippet_session();
        if !self.docs[self.active].view.has_focus(ctx) {
            return;
        }
        if self.snippets.session.is_some() {
            let tab = take_key(ctx, Key::Tab, Modifiers::NONE);
            let back = !tab && take_key(ctx, Key::Tab, Modifiers::SHIFT);
            let esc = !tab && !back && take_key(ctx, Key::Escape, Modifiers::NONE);
            let d = &mut self.docs[self.active];
            let Some((_, s)) = self.snippets.session.as_mut() else { return };
            let more = if tab {
                s.next(&mut d.editor)
            } else if back {
                s.prev(&mut d.editor);
                true
            } else {
                !esc
            };
            if tab || back {
                d.view.reveal();
            }
            if more {
                self.snippet_hint();
            } else {
                self.end_snippet();
            }
        } else if has_key(ctx, Key::Tab, Modifiers::NONE) {
            let d = &self.docs[self.active];
            let found = snippets::trigger_at(&self.snippets.lib, &d.editor, |pos| d.highlighter.scope_at(&d.editor.buffer, pos)).cloned();
            if let Some(s) = found {
                take_key(ctx, Key::Tab, Modifiers::NONE);
                self.insert_snippet(&s, &s.trigger);
            }
        }
    }

    /// After drawing: a snippet clicked in the completion list.
    pub(super) fn snippet_from_completion(&mut self) {
        if let Some((s, prefix)) = self.completion.take_snippet() {
            self.insert_snippet(&s, &prefix);
        }
    }

    /// Expand `s` at every cursor of the current tab, replacing `prefix`.
    pub(super) fn insert_snippet(&mut self, s: &Snippet, prefix: &str) {
        self.end_snippet();
        let d = &mut self.docs[self.active];
        let session = snippets::expand(&mut d.editor, &s.content, prefix, d.path.as_deref());
        d.view.reveal();
        self.snippets.session = session.map(|s| (d.view.id, s));
        self.snippet_hint();
    }

    /// Status bar: which field, and the keys.
    fn snippet_hint(&mut self) {
        if let Some((_, s)) = &self.snippets.session {
            let (field, count) = s.position();
            self.status = format!("{HINT} {field} of {count}: Tab next, Shift+Tab back, Esc done");
        }
    }

    fn check_snippet_session(&mut self) {
        let d = &self.docs[self.active];
        if let Some((id, s)) = &self.snippets.session
            && (*id != d.view.id || !s.alive(&d.editor))
        {
            self.end_snippet();
        }
    }

    fn end_snippet(&mut self) {
        if let Some((id, s)) = self.snippets.session.take()
            && let Some(d) = self.docs.iter_mut().find(|d| d.view.id == id)
        {
            s.end(&mut d.editor);
        }
        if self.status.starts_with(HINT) {
            self.status.clear();
        }
    }

    fn report_snippet_errors(&mut self) {
        if let Some(e) = self.snippets.lib.errors.first() {
            self.status = format!("Snippet error: {e}");
        }
    }

    /// Tools ▸ Snippets…: the snippets valid at the caret; choosing inserts one.
    pub(super) fn show_snippets(&mut self, ctx: &egui::Context) {
        self.snippets.lib.refresh();
        let d = &self.docs[self.active];
        let scopes = d.highlighter.scope_at(&d.editor.buffer, d.editor.primary().start());
        let list: Vec<Snippet> = self.snippets.lib.available(&scopes).into_iter().cloned().collect();
        if list.is_empty() {
            self.status = format!("No snippets for {}", d.highlighter.syntax_name());
            return;
        }
        let labels = list
            .iter()
            .map(|s| if s.trigger.is_empty() { s.label() } else { format!("{}  —  {}", s.label(), s.trigger) })
            .collect();
        self.choose(ctx, Choosing::Snippet(list), "Insert snippet", labels);
    }

    pub(super) fn chose_snippet(&mut self, s: Snippet) {
        self.insert_snippet(&s, "");
    }

    /// Tools ▸ Developer ▸ New Snippet…: a template in a new tab that saves
    /// into the Snippets folder.
    pub(super) fn new_snippet(&mut self) {
        let mut d = Document::from_buffer(None, Buffer::from_str(TEMPLATE));
        d.highlighter.set_syntax("XML");
        let line = d.editor.buffer.line_start(2);
        d.editor.select_ranges(&[line..d.editor.buffer.line_end(2)], 0);
        self.snippets.template_tabs.push(d.view.id);
        self.push_doc(d);
        self.doc().view.focus();
        self.status = match self.snippets.lib.dir() {
            Some(dir) => format!("Save as a .sublime-snippet in {} to use it", dir.display()),
            None => "Save as a .sublime-snippet file".into(),
        };
    }

    /// The Save As dialog for tab `i`: New Snippet tabs default to the Snippets folder.
    pub(super) fn save_dialog(&self, i: usize) -> rfd::FileDialog {
        let d = &self.docs[i];
        let dialog = rfd::FileDialog::new();
        match self.snippets.lib.dir() {
            Some(dir) if d.path.is_none() && self.snippets.template_tabs.contains(&d.view.id) => {
                let _ = std::fs::create_dir_all(dir);
                dialog.set_directory(dir).set_file_name("untitled.sublime-snippet")
            }
            _ => dialog.set_file_name(d.name()),
        }
    }

    /// A file was saved: snippets take effect right away.
    pub(super) fn snippet_saved(&mut self, path: &std::path::Path) {
        if path.extension().is_some_and(|e| e == "sublime-snippet") {
            self.snippets.lib.reload();
            match self.snippets.lib.errors.iter().find(|e| e.starts_with(&path.display().to_string())) {
                Some(e) => self.status = format!("Snippet error: {e}"),
                None if self.snippets.lib.dir().is_some_and(|d| path.starts_with(d)) => self.status = format!("Saved snippet {}", path.display()),
                None => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::Cmd;
    use crate::editor::Editor;
    use egui::{Context, Pos2, RawInput, Rect, vec2};
    use std::path::PathBuf;

    fn frame(app: &mut StrataApp, ctx: &Context, events: Vec<Event>) {
        let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 600.0))), events, ..Default::default() };
        let mut out = ctx.run_ui(raw, |ui| {
            let mut frame = eframe::Frame::_new_kittest();
            eframe::App::ui(app, ui, &mut frame);
        });
        out.textures_delta.clear();
    }

    fn key(key: Key, modifiers: Modifiers) -> Event {
        Event::Key { key, physical_key: Some(key), pressed: true, repeat: false, modifiers }
    }

    fn text(app: &StrataApp) -> String {
        let e = &app.docs[app.active].editor;
        e.buffer.slice(0..e.buffer.len_chars())
    }

    fn rust_app(src: &str) -> (StrataApp, Context) {
        let mut app = StrataApp::new(None, None);
        app.settings.auto_complete = false;
        let d = &mut app.docs[0];
        d.editor = Editor::new(Buffer::from_str(src));
        d.path = Some(PathBuf::from("/tmp/demo/main.rs"));
        d.highlighter.detect(d.path.as_deref(), "");
        let n = d.editor.buffer.len_chars();
        d.editor.select_ranges(&[n..n], 0);
        d.view.focus();
        let ctx = Context::default();
        frame(&mut app, &ctx, vec![]);
        frame(&mut app, &ctx, vec![]);
        (app, ctx)
    }

    #[test]
    fn typed_trigger_and_tabs_walk_the_fields() {
        let (mut app, ctx) = rust_app("");
        assert!(app.docs[0].view.has_focus(&ctx));
        frame(&mut app, &ctx, vec![Event::Text("fn".into())]);
        frame(&mut app, &ctx, vec![key(Key::Tab, Modifiers::NONE)]);
        assert_eq!(text(&app), "fn name() {\n    \n}");
        assert_eq!(app.docs[0].editor.primary().range(), 3..7, "the name placeholder is selected");
        frame(&mut app, &ctx, vec![Event::Text("go".into())]);
        frame(&mut app, &ctx, vec![key(Key::Tab, Modifiers::NONE)]);
        frame(&mut app, &ctx, vec![Event::Text("x: u8".into())]);
        frame(&mut app, &ctx, vec![key(Key::Tab, Modifiers::SHIFT)]);
        assert_eq!(app.docs[0].editor.primary().range(), 3..5, "Shift+Tab goes back to the name");
        frame(&mut app, &ctx, vec![key(Key::Tab, Modifiers::NONE)]);
        assert!(app.snippets.session.is_some());
        frame(&mut app, &ctx, vec![key(Key::Tab, Modifiers::NONE)]);
        assert!(app.snippets.session.is_none(), "the last Tab reaches $0 and ends the snippet");
        frame(&mut app, &ctx, vec![Event::Text("body".into())]);
        assert_eq!(text(&app), "fn go(x: u8) {\n    body\n}");

        // No session and no trigger: Tab indents as usual.
        frame(&mut app, &ctx, vec![key(Key::Tab, Modifiers::NONE)]);
        assert_eq!(text(&app), "fn go(x: u8) {\n    body    \n}");
    }

    #[test]
    fn escape_ends_the_session_and_triggers_respect_scope() {
        let (mut app, ctx) = rust_app("");
        frame(&mut app, &ctx, vec![Event::Text("impl".into())]);
        frame(&mut app, &ctx, vec![key(Key::Tab, Modifiers::NONE)]);
        assert!(app.snippets.session.is_some());
        frame(&mut app, &ctx, vec![key(Key::Escape, Modifiers::NONE)]);
        assert!(app.snippets.session.is_none());
        assert_eq!(app.docs[0].editor.primary().range(), 5..9, "Esc keeps the selection");

        // A Python trigger does nothing in Rust: Tab indents.
        let (mut app, ctx) = rust_app("");
        frame(&mut app, &ctx, vec![Event::Text("def".into()), key(Key::Tab, Modifiers::NONE)]);
        assert_eq!(text(&app), "def ");
    }

    #[test]
    fn completion_popup_lists_and_inserts_snippets() {
        let (mut app, ctx) = rust_app("let testing = 1;\n");
        app.settings.auto_complete = true;
        frame(&mut app, &ctx, vec![Event::Text("te".into())]);
        assert!(app.completion.is_open());
        assert!(app.completion.has_snippet("test"), "the test snippet is offered");
        frame(&mut app, &ctx, vec![Event::Text("st".into())]);
        // "test" matches the trigger exactly, so it comes first; Tab accepts it.
        frame(&mut app, &ctx, vec![key(Key::Tab, Modifiers::NONE)]);
        assert_eq!(text(&app), "let testing = 1;\n#[test]\nfn name() {\n    \n}");
        assert!(app.snippets.session.is_some());
    }

    #[test]
    fn user_snippets_load_and_new_snippet_opens_a_template() {
        let dir = std::env::temp_dir().join(format!("strata-snippets-ui-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Snippets/rust")).unwrap();
        std::fs::write(
            dir.join("Snippets/rust/hi.sublime-snippet"),
            "<snippet><content><![CDATA[println!(\"hi ${1:there}\");]]></content><tabTrigger>hi</tabTrigger><scope>source.rust</scope><description>Say hi</description></snippet>",
        )
        .unwrap();
        let mut app = StrataApp::new(None, Some(dir.clone()));
        let d = &mut app.docs[0];
        d.path = Some(dir.join("a.rs"));
        d.highlighter.detect(d.path.as_deref(), "");
        assert!(app.snippets.lib.snippets.iter().any(|s| s.trigger == "hi"));
        let ctx = Context::default();
        app.show_snippets(&ctx);
        assert!(matches!(&app.choosing, Choosing::Snippet(list) if list[0].trigger == "hi" && list.iter().all(|s| s.scope != "source.python")));
        app.chose(0);
        assert_eq!(text(&app), "println!(\"hi there\");");

        app.run(&ctx, Cmd::NewSnippet);
        assert_eq!(app.docs.len(), 2);
        assert!(text(&app).contains("<tabTrigger>hello</tabTrigger>"));
        assert_eq!(app.doc().highlighter.syntax_name(), "XML");
        let parsed = snippets::parse_xml(&text(&app)).unwrap();
        assert_eq!(parsed.content, "Hello, ${1:this} is a ${2:snippet}.$0");
        std::fs::remove_dir_all(dir).ok();
    }
}
