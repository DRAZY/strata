//! The app's side of key bindings: key presses through the keymap,
//! keybindings.json reloading, and Preferences ▸ Key Bindings (the defaults
//! and the user's file side by side, like Sublime).

use super::StrataApp;
use crate::buffer::Buffer;
use crate::document::Document;
use crate::editor::Editor;
use crate::keybindings::{self, Chord, KeyContext, KeymapFile, Target, What};
use crate::menus::Act;
use crate::panes::Layout;
use std::path::Path;

pub struct KeymapState {
    file: KeymapFile,
    chord: Chord,
    /// Problems found loading the file at startup, for the status bar.
    startup: Option<String>,
    /// Clipboard text of a Paste event turned back into a key press (see
    /// `clipboard_keys`), for Paste and Indent this frame.
    pub pasted: Option<String>,
    /// Modifiers held at the end of the last frame.
    last_mods: egui::Modifiers,
}

impl KeymapState {
    /// Load keybindings.json (if any) and make it the keymap in effect.
    pub fn new(config: Option<&Path>) -> Self {
        let mut file = KeymapFile::new(config);
        let startup = file.load(true);
        Self { file, chord: Chord::default(), startup, pasted: None, last_mods: egui::Modifiers::NONE }
    }

    /// A chord's first key was pressed; the next key finishes it.
    #[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
    pub fn chord_pending(&self) -> bool {
        self.chord.pending()
    }
}

impl StrataApp {
    pub(super) fn keymap_startup_status(&mut self) {
        if let Some(msg) = self.keymap.startup.take() {
            self.status = if self.status.is_empty() { msg } else { format!("{}; {msg}", self.status) };
        }
    }

    /// keybindings.json saved here or edited elsewhere: pick it up. A broken
    /// file keeps the keymap that was working and says what's wrong.
    pub(super) fn watch_keymap(&mut self) {
        if let Some(msg) = self.keymap.file.poll() {
            self.status = msg;
        }
    }

    /// Run the commands bound to this frame's key presses.
    /// What has the keyboard, for bindings' contexts.
    fn key_context(&self, ctx: &egui::Context) -> KeyContext {
        let terminal = self.terminal_has_keys(ctx);
        let editor = !terminal && self.docs[self.active].view.has_focus(ctx);
        KeyContext {
            editor,
            terminal,
            panel_focus: terminal || (self.find.open && !editor && !self.palette.open),
            panel_visible: self.find.open || self.terminal.visible,
            overlay: self.palette.open,
        }
    }

    /// egui-winit turns primary+C/X/V (Shift included) into Copy, Cut and
    /// Paste events before any key press is seen, so a key binding on those
    /// keys never fired, and Ctrl+Shift+V (Paste and Indent) pasted plainly.
    /// Where a binding takes the key right now, put the key press back; a
    /// Paste's text is kept for Paste and Indent. Runs in every window's
    /// frame, before the keymap (eframe's raw input hook only sees the first window).
    fn clipboard_keys(&mut self, ctx: &egui::Context, kc: &KeyContext) {
        use egui::{Event, Key};
        let keymap = keybindings::current();
        let (chord, before) = (&self.keymap.chord, self.keymap.last_mods);
        let pasted = ctx.input_mut(|i| {
            let mut pasted = None;
            // The modifiers as each event happened, starting from the last frame's.
            let mut mods = before;
            for ev in &mut i.events {
                let key = match ev {
                    Event::ModifiersChanged(m) => {
                        mods = *m;
                        continue;
                    }
                    Event::Copy => Key::C,
                    Event::Cut => Key::X,
                    Event::Paste(_) => Key::V,
                    _ => continue,
                };
                // Shift+Insert and friends on Windows, or a menu's Copy/Paste: leave them.
                if !mods.command || !keymap.takes(mods, key, kc, chord) {
                    continue;
                }
                if let Event::Paste(text) = ev {
                    pasted = Some(std::mem::take(text));
                }
                *ev = Event::Key { key, physical_key: Some(key), pressed: true, repeat: false, modifiers: mods };
            }
            pasted
        });
        self.keymap.last_mods = ctx.input(|i| i.modifiers);
        if pasted.is_some() {
            self.keymap.pasted = pasted;
        }
    }

    pub(super) fn dispatch_keys(&mut self, ctx: &egui::Context) {
        let kc = self.key_context(ctx);
        self.clipboard_keys(ctx, &kc);
        let keymap = keybindings::current();
        for fired in keymap.dispatch(ctx, &kc, &mut self.keymap.chord) {
            let Some(target) = fired.action else { continue };
            // Macros record the editor's keys; window commands aren't part of them.
            if target.needs_editor()
                && let Some(rec) = self.macro_recording.as_mut()
            {
                rec.extend(fired.events);
            }
            self.run_target(ctx, target);
        }
    }

    pub(super) fn run_target(&mut self, ctx: &egui::Context, target: Target) {
        match target {
            Target::Cmd(c) => self.run(ctx, c),
            Target::Editor(id) => self.run_editor(id),
            Target::View(name) => self.run_act(ctx, Act::Named(name.into())),
            Target::FoldLevel(n) => self.run_act(ctx, Act::Named(format!("fold_level:{n}"))),
            Target::Tab(n) => {
                let tabs = self.group_tabs(self.focus_group);
                if let Some(&i) = if n == 8 { tabs.last() } else { tabs.get(n as usize) } {
                    self.activate(i);
                }
            }
        }
    }

    /// A command by its keybindings.json id (dev aid: STRATA_DEV_PALETTE="run:key_bindings").
    pub(super) fn run_command_id(&mut self, ctx: &egui::Context, id: &str) {
        match keybindings::resolve(id, &serde_json::Map::new()) {
            Ok(What::Run(t)) => self.run_target(ctx, t),
            Ok(_) => {}
            Err(e) => self.status = e,
        }
    }

    /// Preferences ▸ Key Bindings: the defaults (read-only) on the left and
    /// keybindings.json on the right, created with a short guide if missing.
    pub(super) fn open_key_bindings(&mut self) {
        let Some(path) = self.keymap.file.path().map(Path::to_path_buf) else {
            self.status = "Key bindings need a settings folder".into();
            return;
        };
        if !path.exists() {
            let written = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&path, keybindings::template()));
            if let Err(e) = written {
                self.status = format!("Could not create {}: {e}", path.display());
                return;
            }
            // Nothing to apply yet; just note the new file so it doesn't read as a change.
            self.keymap.file.load(true);
        }
        if self.layout.groups() < 2 {
            self.set_layout(Layout::grid(2, 1));
        }
        let text = keybindings::defaults_text();
        let defaults = self.docs.iter().position(|d| d.path.is_none() && d.title.as_deref() == Some(keybindings::DEFAULTS_TITLE));
        match defaults {
            Some(i) => {
                // Refresh it (the platform's defaults don't change, but Strata's might have).
                let d = &mut self.docs[i];
                d.editor = Editor::new(Buffer::from_str(&text));
                d.editor.read_only = true;
                d.group = 0;
                self.activate(i);
            }
            None => {
                let mut doc = Document::from_buffer(None, Buffer::from_str(&text));
                doc.title = Some(keybindings::DEFAULTS_TITLE.into());
                doc.editor.read_only = true;
                doc.highlighter.set_syntax("JSON");
                self.focus_group = 0;
                self.push_doc(doc);
            }
        }
        self.focus_group = 1;
        self.open_path(path.clone(), false);
        if let Some(i) = self.docs.iter().position(|d| d.path.as_ref() == Some(&path)) {
            self.activate(i);
            if self.docs[i].group != 1 {
                self.move_to_pane(1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::Cmd;
    use crate::keys::*;
    use crate::menus::Item;
    use egui::{Context, Event, Key, Modifiers, Pos2, RawInput, Rect, vec2};
    use std::path::PathBuf;

    fn temp_config(tag: &str, keymap: Option<&str>) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("strata-keymap-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        if let Some(k) = keymap {
            std::fs::write(dir.join(keybindings::FILE_NAME), k).unwrap();
        }
        dir
    }

    /// Modifiers the way the windowing layer reports them for a pattern.
    fn held(pattern: Modifiers) -> Modifiers {
        let mut m = pattern;
        if MAC {
            m.mac_cmd |= pattern.command;
        } else {
            let c = pattern.ctrl || pattern.command;
            (m.ctrl, m.command) = (c, c);
        }
        m
    }

    fn press(key: Key, pattern: Modifiers) -> Event {
        Event::Key { key, physical_key: Some(key), pressed: true, repeat: false, modifiers: held(pattern) }
    }

    /// One full frame of the real app, with raw input events.
    fn frame(app: &mut StrataApp, ctx: &Context, events: Vec<Event>) {
        let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 600.0))), events, ..Default::default() };
        let mut out = ctx.run_ui(raw, |ui| {
            let mut frame = eframe::Frame::_new_kittest();
            eframe::App::ui(app, ui, &mut frame);
        });
        out.textures_delta.clear();
    }

    fn app_with(config: Option<PathBuf>, text: &str) -> (StrataApp, Context) {
        let mut app = StrataApp::new(None, config);
        app.docs[0].editor = Editor::new(Buffer::from_str(text));
        let ctx = Context::default();
        frame(&mut app, &ctx, vec![]);
        frame(&mut app, &ctx, vec![]);
        assert!(app.docs[app.active].view.has_focus(&ctx), "the editor has the keyboard");
        (app, ctx)
    }

    fn text(app: &StrataApp) -> String {
        let e = &app.docs[app.active].editor;
        e.buffer.slice(0..e.buffer.len_chars())
    }

    #[test]
    fn rebound_command_palette_follows_the_new_key_only() {
        let config = temp_config("palette", Some(r#"[ { "keys": ["ctrl+alt+p"], "command": "command_palette" } ]"#));
        let (mut app, ctx) = app_with(Some(config.clone()), "");
        frame(&mut app, &ctx, vec![press(Key::P, CMD_SHIFT)]);
        assert!(!app.palette.open, "the old key no longer opens the palette");
        frame(&mut app, &ctx, vec![press(Key::P, CTRL_ALT)]);
        assert!(app.palette.open, "the new key does");
        std::fs::remove_dir_all(config).ok();
    }

    #[test]
    fn remapped_editor_commands_and_chords_through_the_input_path() {
        let config = temp_config(
            "editor",
            Some(
                r#"[
                    { "keys": ["ctrl+alt+shift+d"], "command": "duplicate_line" },
                    { "keys": ["primary+k", "primary+t"], "command": "title_case" },
                ]"#,
            ),
        );
        let (mut app, ctx) = app_with(Some(config.clone()), "one two\n");
        frame(&mut app, &ctx, vec![press(Key::D, CMD_SHIFT)]);
        assert_eq!(text(&app), "one two\n", "the default key is free now");
        frame(&mut app, &ctx, vec![press(Key::D, CTRL_ALT.plus(Modifiers::SHIFT))]);
        assert_eq!(text(&app), "one two\none two\n");
        // A user chord, split across frames like real typing.
        app.doc().editor.select_ranges(&[0..7], 0);
        frame(&mut app, &ctx, vec![press(Key::K, CMD)]);
        assert!(app.keymap.chord_pending());
        frame(&mut app, &ctx, vec![press(Key::T, CMD)]);
        assert_eq!(text(&app), "One Two\none two\n");
        // The default chords still work.
        frame(&mut app, &ctx, vec![press(Key::K, CMD), press(Key::U, CMD)]);
        assert_eq!(text(&app), "ONE TWO\none two\n");
        std::fs::remove_dir_all(config).ok();
    }

    #[test]
    fn default_editor_keys_reach_the_editor() {
        let (mut app, ctx) = app_with(None, "let a = a + a;\n    let x = 1;\n");
        app.doc().highlighter.set_syntax("Rust");
        app.doc().editor.select_ranges(&[4..4], 0);
        frame(&mut app, &ctx, vec![press(Key::D, CMD); 3]);
        assert_eq!(app.doc().editor.selections().len(), 3, "⌘D thrice");
        frame(&mut app, &ctx, vec![Event::Text("b".into())]);
        assert_eq!(text(&app), "let b = b + b;\n    let x = 1;\n");
        let line2 = app.doc().editor.buffer.line_start(1) + 4;
        app.doc().editor.select_ranges(&[line2..line2], 0);
        frame(&mut app, &ctx, vec![press(Key::Slash, CMD)]);
        assert_eq!(text(&app), "let b = b + b;\n    // let x = 1;\n");
        let move_mods = if MAC { CTRL_CMD } else { CTRL_SHIFT };
        frame(&mut app, &ctx, vec![press(Key::ArrowUp, move_mods)]);
        assert_eq!(text(&app), "    // let x = 1;\nlet b = b + b;\n");
        frame(&mut app, &ctx, vec![press(Key::Z, CMD)]);
        assert_eq!(text(&app), "let b = b + b;\n    // let x = 1;\n", "undo");
        // Folding chords: ⌘K ⌘1 folds all, ⌘K ⌘J unfolds.
        let (mut app, ctx) = app_with(None, "class A:\n    def f(self):\n        return 1\n");
        frame(&mut app, &ctx, vec![press(Key::K, CMD), press(Key::Num1, CMD)]);
        assert!(app.doc().view.is_folded_line(1));
        frame(&mut app, &ctx, vec![press(Key::K, CMD), press(Key::J, CMD)]);
        assert!(!app.doc().view.is_folded_line(1));
    }

    #[test]
    fn editor_keys_leave_text_fields_alone() {
        let (mut app, ctx) = app_with(None, "aaa aaa\n");
        app.run(&ctx, Cmd::Find);
        frame(&mut app, &ctx, vec![]);
        frame(&mut app, &ctx, vec![]);
        assert!(!app.docs[app.active].view.has_focus(&ctx), "the find field has the keyboard");
        frame(&mut app, &ctx, vec![press(Key::D, CMD)]);
        assert_eq!(app.doc().editor.selections().len(), 1, "⌘D didn't reach the editor");
    }

    #[test]
    fn unbinding_and_tab_digits() {
        let config = temp_config("unbind", Some(r#"[ { "keys": ["primary+shift+p"], "command": "" } ]"#));
        let (mut app, ctx) = app_with(Some(config.clone()), "");
        frame(&mut app, &ctx, vec![press(Key::P, CMD_SHIFT)]);
        assert!(!app.palette.open);
        app.push_doc(Document::untitled());
        app.push_doc(Document::untitled());
        assert_eq!(app.active, 2);
        frame(&mut app, &ctx, vec![press(Key::Num1, if MAC { CMD } else { ALT })]);
        assert_eq!(app.active, 0);
        frame(&mut app, &ctx, vec![press(Key::Num9, if MAC { CMD } else { ALT })]);
        assert_eq!(app.active, 2, "9 is the last tab");
        std::fs::remove_dir_all(config).ok();
    }

    fn find_item<'a>(items: &'a [Item], name: &str) -> Option<&'a Item> {
        items.iter().find_map(|it| match it {
            Item::Action { label, .. } if label == name => Some(it),
            Item::Sub(_, kids) => find_item(kids, name),
            _ => None,
        })
    }

    fn shortcut(app: &StrataApp, name: &str) -> (Option<(Modifiers, Key)>, Option<String>) {
        let menus = app.menus();
        let item = menus.iter().find_map(|(_, items)| find_item(items, name)).unwrap_or_else(|| panic!("no menu item {name}"));
        match item {
            Item::Action { keys, hint, .. } => (*keys, hint.clone()),
            _ => unreachable!(),
        }
    }

    #[test]
    fn menus_show_the_keymap() {
        let (app, _) = app_with(None, "");
        assert_eq!(shortcut(&app, "Command Palette…"), (Some((CMD_SHIFT, Key::P)), None));
        assert_eq!(shortcut(&app, "Upper Case").1, Some(format!("{} {}", label(CMD, Key::K), label(CMD, Key::U))));
        drop(app);
        let config = temp_config(
            "menus",
            Some(
                r#"[
                    { "keys": ["ctrl+alt+shift+p"], "command": "show_overlay", "args": {"overlay": "command_palette"} },
                    { "keys": ["primary+shift+u"], "command": "upper_case" },
                    { "keys": ["primary+k", "primary+d"], "command": "duplicate_line" },
                    { "keys": ["primary+c"], "command": "noop" },
                ]"#,
            ),
        );
        let (app, _) = app_with(Some(config.clone()), "");
        assert_eq!(shortcut(&app, "Command Palette…"), (Some((CTRL_ALT.plus(Modifiers::SHIFT), Key::P)), None));
        assert_eq!(shortcut(&app, "Upper Case"), (Some((CMD_SHIFT, Key::U)), None));
        assert_eq!(shortcut(&app, "Soft Redo"), (None, None), "⌘⇧U went to Upper Case");
        assert_eq!(shortcut(&app, "Duplicate Line"), (None, Some(format!("{} {}", label(CMD, Key::K), label(CMD, Key::D)))));
        assert_eq!(shortcut(&app, "Copy"), (None, None), "a binding on ⌘C takes the key from the menu");
        assert!(app.status.contains("soft_redo"), "shadowing is reported: {}", app.status);
        std::fs::remove_dir_all(config).ok();
    }

    #[test]
    fn key_bindings_open_side_by_side_and_reload_live() {
        let config = temp_config("open", None);
        let (mut app, ctx) = app_with(Some(config.clone()), "");
        app.run(&ctx, Cmd::KeyBindings);
        let path = config.join(keybindings::FILE_NAME);
        assert!(std::fs::read_to_string(&path).unwrap().contains("Strata key bindings"));
        assert_eq!(app.layout.groups(), 2);
        let defaults = app.docs.iter().find(|d| d.name() == keybindings::DEFAULTS_TITLE).expect("defaults tab");
        assert!(defaults.editor.read_only);
        assert_eq!(defaults.group, 0);
        assert!(defaults.editor.buffer.slice(0..defaults.editor.buffer.len_chars()).contains("\"command\": \"command_palette\""));
        assert_eq!(app.doc().path.as_deref(), Some(path.as_path()));
        assert_eq!(app.doc().group, 1);
        // Again: no second defaults tab.
        app.run(&ctx, Cmd::KeyBindings);
        assert_eq!(app.docs.iter().filter(|d| d.name() == keybindings::DEFAULTS_TITLE).count(), 1);

        // Saved with a typo: the status says where, the old keymap stays.
        std::fs::write(&path, "[\n  { \"keys\": [\"ctrl+alt+p\"], \"command\": \"command_palette\" },\n]").unwrap();
        app.watch_keymap();
        assert_eq!(Cmd::CommandPalette.keys(), [(CTRL_ALT, Key::P)]);
        std::fs::write(&path, "[\n  { \"keys\": [\"ctrl+alt+o\"] \"command\": \"command_palette\" },\n]").unwrap();
        app.watch_keymap();
        assert!(app.status.contains("line 2") && app.status.contains("keeping"), "{}", app.status);
        assert_eq!(Cmd::CommandPalette.keys(), [(CTRL_ALT, Key::P)]);
        frame(&mut app, &ctx, vec![]);
        std::fs::remove_dir_all(config).ok();
    }
}
