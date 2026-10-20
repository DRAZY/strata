//! The app's side of the terminal panel: toggling, new tabs, layout, Send to
//! Terminal, opening clicked file locations, and which window shortcuts
//! still work while a shell has the keyboard.

use super::StrataApp;
use crate::editor::Editor;
use crate::terminal::{self, PanelAction};

/// What Send to Terminal sends: the selected text (several selections one
/// after another, each on its own line), or with nothing selected the lines
/// the cursors are on.
pub(super) fn send_text(e: &Editor) -> String {
    let b = &e.buffer;
    let sels = e.selections();
    if sels.iter().any(|s| !s.is_empty()) {
        return sels.iter().filter(|s| !s.is_empty()).map(|s| b.slice(s.start()..s.end())).collect::<Vec<_>>().join("\n");
    }
    let mut lines: Vec<usize> = sels.iter().map(|s| b.line_of(s.head)).collect();
    lines.sort_unstable();
    lines.dedup();
    lines.into_iter().map(|l| b.line_text(l)).collect::<Vec<_>>().join("\n")
}

impl StrataApp {
    pub(super) fn terminal_dir(&self) -> std::path::PathBuf {
        // The folder holding the current file, else the first folder.
        let file = self.docs[self.active].path.as_deref();
        let folder = file.and_then(|f| self.sidebar.root_of(f)).or(self.sidebar.root());
        terminal::start_dir(folder, file)
    }

    pub(super) fn set_terminal_visible(&mut self, on: bool) {
        self.terminal.visible = on;
        if self.settings.terminal_visible != on {
            self.settings.terminal_visible = on;
            self.persist_settings();
        }
    }

    pub(super) fn new_terminal(&mut self, ctx: &egui::Context) {
        let dir = self.terminal_dir();
        match self.terminal.open(ctx, &dir) {
            Ok(()) => {
                self.set_terminal_visible(true);
                self.terminal.focus();
            }
            Err(e) => {
                log::warn!("couldn't start a terminal in {}: {e}", dir.display());
                self.status = e;
            }
        }
    }

    /// Send the selection (or the current line) to the active terminal,
    /// starting one if there is none. The editor keeps the keyboard, so you
    /// can go on sending lines to a REPL.
    pub(super) fn send_to_terminal(&mut self, ctx: &egui::Context) {
        let text = send_text(&self.docs[self.active].editor);
        if self.terminal.tabs.is_empty() {
            let dir = self.terminal_dir();
            if let Err(e) = self.terminal.open(ctx, &dir) {
                log::warn!("couldn't start a terminal in {}: {e}", dir.display());
                self.status = e;
                return;
            }
        }
        self.set_terminal_visible(true);
        if let Some(t) = self.terminal.active_terminal_mut() {
            t.send(&text);
        }
    }

    pub(super) fn toggle_terminal(&mut self, ctx: &egui::Context) {
        if self.terminal.visible {
            self.set_terminal_visible(false);
            self.doc().view.focus();
        } else if self.terminal.tabs.is_empty() {
            self.new_terminal(ctx);
        } else {
            self.set_terminal_visible(true);
            self.terminal.focus();
        }
    }

    /// Window shortcuts while the terminal has the keyboard (it owns Ctrl/Alt keys).
    pub(super) fn terminal_has_keys(&self, ctx: &egui::Context) -> bool {
        self.terminal.has_focus(ctx)
    }

    /// Poll shells and draw the bottom panel. Call before the central panel.
    pub(super) fn terminal_panel(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if !self.terminal_started {
            self.terminal_started = true;
            // Dev aid: types a command into a new terminal (debug builds only; see `crate::dev`).
            let dev = crate::dev::risky_var("STRATA_DEV_TERMINAL");
            if self.settings.terminal_visible || dev.is_some() {
                self.new_terminal(&ctx);
                if let (Some(cmd), Some(t)) = (dev, self.terminal.active_terminal()) {
                    t.write(format!("{cmd}\r"));
                    self.doc().view.cancel_focus();
                }
            }
        }
        self.terminal.clipboard = crate::terminal::Clipboard { read: self.settings.terminal_allow_clipboard_read, write: self.settings.terminal_allow_clipboard_write };
        self.terminal.option_as_meta = self.settings.terminal_option_as_meta;
        self.terminal.roots.clone_from(&self.sidebar.roots);
        let all_exited = self.terminal.poll(&ctx);
        let clip = std::mem::take(&mut self.terminal.clipboard_use);
        if clip.read_blocked {
            self.status = "A program in the terminal tried to read the clipboard; Strata blocked it (setting: terminal_allow_clipboard_read)".into();
        } else if clip.write_blocked {
            self.status = "A program in the terminal tried to set the clipboard; Strata blocked it (setting: terminal_allow_clipboard_write)".into();
        } else if clip.wrote {
            self.status = "A program in the terminal set the clipboard".into();
        }
        if all_exited {
            // The last shell exited.
            self.set_terminal_visible(false);
            self.doc().view.focus();
        }
        if !self.terminal.visible || self.distraction.is_some() {
            return;
        }
        let theme = terminal::Theme::from_highlighter(&self.docs[self.active].highlighter);
        let font_size = self.settings.font_size;
        self.terminal.opacity = self.settings.window_opacity.clamp(0.3, 1.0);
        let out = egui::Panel::bottom(self.link.id("terminal"))
            .resizable(true)
            .default_size(self.settings.terminal_height.clamp(80.0, 2000.0))
            .min_size(60.0)
            .frame(egui::Frame::side_top_panel(ui.style()).fill(egui::Color32::TRANSPARENT))
            .show(ui, |ui| self.terminal.show(ui, &theme, font_size));
        self.terminal_context_menus();
        match out.inner {
            Some(PanelAction::Hide) => {
                self.set_terminal_visible(false);
                self.doc().view.focus();
            }
            Some(PanelAction::NewTab) => self.new_terminal(&ctx),
            Some(PanelAction::Open(path, line, col)) => self.open_at(path, line, col..col),
            None => {}
        }
        // Remember the height once a resize drag ends.
        let h = out.response.rect.height().round();
        if (h - self.settings.terminal_height).abs() >= 1.0 && !ctx.input(|i| i.pointer.any_down()) {
            self.settings.terminal_height = h;
            self.persist_settings();
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::commands::Cmd;
    use egui::{Event, Key, Modifiers, Pos2, RawInput, Rect, vec2};

    fn frame(app: &mut StrataApp, ctx: &egui::Context, events: Vec<Event>) -> egui::PlatformOutput {
        frame_held(app, ctx, events, Modifiers::NONE)
    }

    /// A frame with `modifiers` held down.
    fn frame_held(app: &mut StrataApp, ctx: &egui::Context, mut events: Vec<Event>, modifiers: Modifiers) -> egui::PlatformOutput {
        events.insert(0, Event::ModifiersChanged(modifiers));
        let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 700.0))), events, ..Default::default() };
        let mut out = ctx.run_ui(raw, |ui| {
            let mut frame = eframe::Frame::_new_kittest();
            eframe::App::ui(app, ui, &mut frame);
        });
        out.textures_delta.clear();
        out.platform_output
    }

    fn key(key: Key, modifiers: Modifiers) -> Event {
        Event::Key { key, physical_key: Some(key), pressed: true, repeat: false, modifiers }
    }

    /// An app whose terminal runs `cmd` (echo off, so the screen shows only
    /// what `cat -v` prints back), focused and ready.
    fn shell_app(cmd: &str) -> (egui::Context, StrataApp) {
        let ctx = egui::Context::default();
        let mut app = StrataApp::new(None, None);
        frame(&mut app, &ctx, vec![]);
        let shell = ("/bin/sh".to_owned(), vec!["-c".to_owned(), format!("stty -echo; echo READY; {cmd}")]);
        app.terminal.tabs.push(crate::terminal::Terminal::spawn(Some(ctx.clone()), &std::env::temp_dir(), Some(shell), 80, 10).unwrap());
        app.terminal.visible = true;
        app.terminal.focus();
        wait_for(&mut app, &ctx, "READY");
        (ctx, app)
    }

    /// Run frames until the screen contains `needle`; returns the screen.
    fn wait_for(app: &mut StrataApp, ctx: &egui::Context, needle: &str) -> String {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            frame(app, ctx, vec![]);
            let screen = app.terminal.active_terminal().unwrap().screen_text();
            if screen.contains(needle) || std::time::Instant::now() > deadline {
                return screen;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    #[test]
    fn option_types_the_layout_character_unless_meta() {
        let (ctx, mut app) = shell_app("cat -v");
        // What macOS sends for Option+L on a German layout: the key, and "@" as text.
        let option_l = || vec![key(Key::L, Modifiers::ALT), Event::Text("@".into()), key(Key::Enter, Modifiers::NONE)];
        app.settings.terminal_option_as_meta = false;
        frame_held(&mut app, &ctx, option_l(), Modifiers::ALT);
        let screen = wait_for(&mut app, &ctx, "@");
        assert!(screen.lines().any(|l| l == "@"), "{screen}");
        app.settings.terminal_option_as_meta = true;
        frame_held(&mut app, &ctx, option_l(), Modifiers::ALT);
        let screen = wait_for(&mut app, &ctx, "^[l");
        assert!(screen.lines().any(|l| l == "^[l"), "Meta sends Esc + l: {screen}");
    }

    #[test]
    fn input_method_text_is_sent_when_committed() {
        let (ctx, mut app) = shell_app("cat -v");
        let preedit = |t: &str| Event::Ime(egui::ImeEvent::Preedit { text: t.into(), active_range_chars: None });
        let out = frame(&mut app, &ctx, vec![preedit("n"), preedit("ni")]);
        assert_eq!(app.terminal.active_terminal().unwrap().preedit(), "ni");
        let body = app.terminal.body.as_ref().unwrap().rect;
        assert!(out.ime.is_some_and(|i| body.contains(i.cursor_rect.min) && i.purpose == egui::IMEPurpose::Terminal), "the input method knows where the cursor is");
        // Enter mid-composition belongs to the input method.
        frame(&mut app, &ctx, vec![key(Key::Enter, Modifiers::NONE), Event::Ime(egui::ImeEvent::Commit("你好".into()))]);
        assert!(app.terminal.active_terminal().unwrap().preedit().is_empty());
        frame(&mut app, &ctx, vec![Event::Text("!".into()), key(Key::Enter, Modifiers::NONE)]);
        let screen = wait_for(&mut app, &ctx, "你好");
        assert!(screen.lines().any(|l| l == "你好!"), "{screen}");
    }

    #[test]
    fn programs_that_ask_for_the_mouse_get_clicks() {
        // 1000: report clicks; 1002: and drags; 1006: SGR encoding.
        let (ctx, mut app) = shell_app("printf '\\033[?1002h\\033[?1006h'; cat -v");
        let body = app.terminal.body.as_ref().unwrap().rect;
        let (cw, ch) = app.terminal.active_terminal().unwrap().cell_size();
        // The middle of cell (col 1, row 0), 1-based (2, 1), on "READY".
        let p = body.min + vec2(6.0 + 1.5 * cw, 4.0 + 0.5 * ch);
        let q = p + vec2(3.0 * cw, 0.0);
        let button = |pos, button, pressed, modifiers| Event::PointerButton { pos, button, pressed, modifiers };
        use egui::PointerButton::*;
        // Shift+drag selects locally; nothing reaches the program.
        let shift = Modifiers::SHIFT;
        frame_held(&mut app, &ctx, vec![Event::PointerMoved(p), button(p, Primary, true, shift)], shift);
        frame_held(&mut app, &ctx, vec![Event::PointerMoved(q)], shift);
        frame_held(&mut app, &ctx, vec![button(q, Primary, false, shift)], shift);
        assert!(app.terminal.has_selection(), "Shift+drag selects");
        // A drag, then a Ctrl+right-click.
        frame(&mut app, &ctx, vec![Event::PointerMoved(p), button(p, Primary, true, Modifiers::NONE)]);
        frame(&mut app, &ctx, vec![Event::PointerMoved(q)]);
        frame(&mut app, &ctx, vec![button(q, Primary, false, Modifiers::NONE)]);
        frame_held(&mut app, &ctx, vec![button(p, Secondary, true, Modifiers::CTRL)], Modifiers::CTRL);
        frame_held(&mut app, &ctx, vec![button(p, Secondary, false, Modifiers::CTRL)], Modifiers::CTRL);
        assert!(app.terminal.body.is_none(), "the program took the right-click, so no context menu");
        frame(&mut app, &ctx, vec![key(Key::Enter, Modifiers::NONE)]);
        let screen = wait_for(&mut app, &ctx, "^[[<18;2;1m");
        assert!(screen.contains("^[[<0;2;1M^[[<32;5;1M^[[<0;5;1m^[[<18;2;1M^[[<18;2;1m"), "{screen}");
    }

    /// Terminal scenes for eyeballing the renderer:
    /// `STRATA_TERMINAL_SHOTS=<folder> cargo test --release terminal_shots`.
    #[test]
    fn terminal_shots() {
        let Some(dir) = std::env::var_os("STRATA_TERMINAL_SHOTS").map(std::path::PathBuf::from) else { return };
        let Some(mut sh) = super::super::headless::Shooter::new(900, 420, 2.0) else { return };
        let file = std::env::temp_dir().join("strata-terminal-shot.txt");
        let body: String = (1..=60).map(|i| format!("line {i}\tcol\t中文 {i}\n")).collect();
        std::fs::write(&file, body).unwrap();
        let src = std::env::temp_dir().join("strata-shot-src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("main.rs"), "fn main() {}\n").unwrap();
        let scenes = [
            ("tabs", "printf 'A\\tB 中文\\nname\\tsize\\tdate\\nab\\tc\\td\\n中文\\tX\\tY\\n|1234567|1234567|1234567|\\n\\033[4mun\\tder\\033[0m|\\n'; ls -l / | head -4; echo DONE; sleep 30".to_owned(), "DONE"),
            ("emoji", "printf 'thumbs 👍🏽 |\\nfamily 👨\\342\\200\\215👩\\342\\200\\215👧 |\\nkeycap 1\\357\\270\\217\\342\\203\\243 |\\ncafe\\314\\201 |\\nheart ❤\\357\\270\\217 |\\nok 👍 |\\n'; echo DONE; sleep 30".to_owned(), "DONE"),
            ("less", format!("less {}; sleep 30", file.display()), "line 5"),
            ("ime", "printf 'typing: '; sleep 30".to_owned(), "typing"),
            ("link", "printf 'error[E0425]: cannot find value `y` in this scope\\n  --> strata-shot-src/main.rs:2:5\\n   |\\n2  |     y\\n   |     ^ not found\\n\\nthread main panicked at strata-shot-src/main.rs:10:3:\\n'; echo DONE; sleep 30".to_owned(), "DONE"),
        ];
        for (name, cmd, marker) in scenes {
            let ctx = egui::Context::default();
            let mut app = StrataApp::new(None, None);
            sh.run(&ctx, &mut app, 2, vec![]);
            let shell = ("/bin/sh".to_owned(), vec!["-c".to_owned(), cmd]);
            app.terminal.tabs.push(crate::terminal::Terminal::spawn(Some(ctx.clone()), &std::env::temp_dir(), Some(shell), 80, 12).unwrap());
            app.terminal.visible = true;
            app.terminal.focus();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while std::time::Instant::now() < deadline {
                sh.run(&ctx, &mut app, 1, vec![]);
                if app.terminal.active_terminal().unwrap().screen_text().contains(marker) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            std::thread::sleep(std::time::Duration::from_millis(300));
            // An input method composing Chinese at the cursor.
            let events = match name {
                "ime" => vec![Event::Ime(egui::ImeEvent::Preedit { text: "ni hao 你好".into(), active_range_chars: None })],
                // ⌘ (Ctrl) held over the rustc location underlines it.
                "link" => vec![Event::ModifiersChanged(link_mods()), Event::PointerMoved(screen_pos(&app, "main.rs:2:5", 2))],
                _ => vec![],
            };
            sh.shoot(&ctx, &mut app, 3, events, &dir.join(format!("term_{name}.ppm")));
        }
    }

    #[test]
    fn send_text_is_the_selection_or_the_current_line() {
        use crate::buffer::Buffer;
        let mut e = Editor::new(Buffer::from_str("print(1)\nx = 2\n\nfor i in y:\n    f(i)\n"));
        e.select_ranges(&[11..11], 0);
        assert_eq!(send_text(&e), "x = 2", "no selection: the cursor's line, without its line break");
        e.select_ranges(&[0..3], 0);
        assert_eq!(send_text(&e), "pri");
        e.select_ranges(&[16..37], 0);
        assert_eq!(send_text(&e), "for i in y:\n    f(i)\n", "several lines, as selected");
        e.select_ranges(&[15..15], 0);
        assert_eq!(send_text(&e), "", "an empty line sends just Enter");
        e.select_ranges(&[0..0, 1..1, 9..9], 0);
        assert_eq!(send_text(&e), "print(1)\nx = 2", "each cursor's line once");
        e.select_ranges(&[0..5, 7..7, 9..10], 0);
        assert_eq!(send_text(&e), "print\nx", "selected text wins over empty cursors");
    }

    /// The link modifier held: ⌘ on macOS, Ctrl elsewhere.
    fn link_mods() -> Modifiers {
        if crate::keys::MAC { Modifiers::MAC_CMD | Modifiers::COMMAND } else { Modifiers::CTRL | Modifiers::COMMAND }
    }

    /// Where `needle` is drawn on the screen (the middle of its `nth` character).
    fn screen_pos(app: &StrataApp, needle: &str, nth: usize) -> Pos2 {
        let t = app.terminal.active_terminal().unwrap();
        let screen = t.screen_text();
        let (row, col) = screen.lines().enumerate().find_map(|(r, l)| l.find(needle).map(|c| (r, c))).unwrap_or_else(|| panic!("{needle} not on screen: {screen}"));
        let body = app.terminal.body.as_ref().unwrap().rect;
        let (cw, ch) = t.cell_size();
        body.min + vec2(6.0 + (col + nth) as f32 * cw + 0.5 * cw, 4.0 + (row as f32 + 0.5) * ch)
    }

    /// Hover `p` with the modifier held until the link checked in the
    /// background appears (giving up after a second).
    fn hover(app: &mut StrataApp, ctx: &egui::Context, p: Pos2) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        loop {
            frame_held(app, ctx, vec![Event::PointerMoved(p)], link_mods());
            if app.terminal.active_terminal().unwrap().hovered_link().is_some() || std::time::Instant::now() > deadline {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// Hover `p` with the modifier held, then click there.
    fn link_click(app: &mut StrataApp, ctx: &egui::Context, p: Pos2) {
        let m = link_mods();
        hover(app, ctx, p);
        let button = |pressed| Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers: m };
        frame_held(app, ctx, vec![button(true)], m);
        frame_held(app, ctx, vec![button(false)], m);
        frame(app, ctx, vec![]);
    }

    #[test]
    fn modifier_click_opens_file_locations() {
        let dir = std::env::temp_dir().join(format!("strata-term-click-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        let rust: String = (1..=30).map(|i| format!("// line {i}\n")).collect();
        std::fs::write(dir.join("src/main.rs"), &rust).unwrap();
        std::fs::write(dir.join("src/lib.rs"), &rust).unwrap();
        std::fs::write(dir.join("tool.py"), "import sys\n\n\ndef f():\n    raise ValueError()\n").unwrap();
        // Last, a panic padded so its location wraps after "src/lib" (once the
        // panel has given the terminal its width).
        let script = "stty -echo; echo READY; printf 'src/main.rs:10:3\\n'; printf 'error[E0425]: nope\\n  --> src/lib.rs:7:5\\n'; \
             printf 'Traceback (most recent call last):\\n  File \"tool.py\", line 5, in <module>\\n'; \
             printf 'missing.rs:3:1\\n'; sleep 1; cols=$(stty size | cut -d' ' -f2); \
             printf \"%$((cols - 20))s\" '' | tr ' ' x; printf ' panicked at src/lib.rs:21:9:\\n'; echo DONE; sleep 30"
            .to_owned();
        let ctx = egui::Context::default();
        let mut app = StrataApp::new(Some(dir.clone()), None);
        frame(&mut app, &ctx, vec![]);
        let shell = ("/bin/sh".to_owned(), vec!["-c".to_owned(), script]);
        app.terminal.tabs.push(crate::terminal::Terminal::spawn(Some(ctx.clone()), &dir, Some(shell), 80, 12).unwrap());
        app.terminal.visible = true;
        wait_for(&mut app, &ctx, "DONE");

        // Hovering without the modifier underlines nothing.
        let p = screen_pos(&app, "src/main.rs:10:3", 2);
        frame(&mut app, &ctx, vec![Event::PointerMoved(p)]);
        assert!(app.terminal.active_terminal().unwrap().hovered_link().is_none());
        assert!(app.terminal.active_terminal().unwrap().link_lookup_is_empty(), "no disk work without the modifier");
        // With it, the whole location is underlined.
        hover(&mut app, &ctx, p);
        let (path, line, col, rows) = app.terminal.active_terminal().unwrap().hovered_link().expect("a link under the pointer");
        assert_eq!((path, line, col), (dir.join("src/main.rs").as_path(), 9, 2));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].2 - rows[0].1, "src/main.rs:10:3".len());

        let opened = |app: &mut StrataApp| {
            let d = app.doc();
            (d.path.clone().unwrap(), d.editor.cursor_line_col())
        };
        link_click(&mut app, &ctx, p);
        assert_eq!(opened(&mut app), (dir.join("src/main.rs"), (10, 3)), "path:line:col");
        frame(&mut app, &ctx, vec![]);
        assert!(app.docs[app.active].view.has_focus(&ctx), "the opened file gets the keyboard");

        let p = screen_pos(&app, "src/lib.rs:7:5", 0);
        link_click(&mut app, &ctx, p);
        assert_eq!(opened(&mut app), (dir.join("src/lib.rs"), (7, 5)), "rustc's --> line");

        let p = screen_pos(&app, "tool.py", 1);
        link_click(&mut app, &ctx, p);
        assert_eq!(opened(&mut app), (dir.join("tool.py"), (5, 1)), "a Python traceback");

        // The wrapped panic location, clicked on its second row.
        let p = screen_pos(&app, ":21:9:", 1);
        hover(&mut app, &ctx, p);
        let rows = app.terminal.active_terminal().unwrap().hovered_link().expect("wrapped link").3.to_vec();
        assert_eq!(rows.len(), 2, "underlined on both rows: {rows:?}");
        link_click(&mut app, &ctx, p);
        assert_eq!(opened(&mut app), (dir.join("src/lib.rs"), (21, 9)), "a location wrapped across rows");

        // A file that doesn't exist isn't a link, and clicking it opens nothing.
        let tabs = app.docs.len();
        let p = screen_pos(&app, "missing.rs:3:1", 1);
        hover(&mut app, &ctx, p);
        assert!(app.terminal.active_terminal().unwrap().hovered_link().is_none());
        link_click(&mut app, &ctx, p);
        assert_eq!(app.docs.len(), tabs);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn links_follow_cd_while_the_modifier_stays_held() {
        // The same printed path names another file once the shell has moved.
        let dir = std::env::temp_dir().join(format!("strata-term-cdlink-{}", std::process::id()));
        for sub in ["a", "b"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
            std::fs::write(dir.join(sub).join("foo.rs"), "// one\n// two\n").unwrap();
        }
        let ctx = egui::Context::default();
        let mut app = StrataApp::new(None, None);
        frame(&mut app, &ctx, vec![]);
        let script = "cd a; echo 'first foo.rs:1:1'; read -r go; cd ../b; echo 'second foo.rs:2:1'; sleep 30";
        let shell = ("/bin/sh".to_owned(), vec!["-c".to_owned(), script.to_owned()]);
        app.terminal.tabs.push(crate::terminal::Terminal::spawn(Some(ctx.clone()), &dir, Some(shell), 80, 12).unwrap());
        app.terminal.visible = true;
        wait_for(&mut app, &ctx, "first foo.rs");
        // Hover until the underlined link names `want` (or give up after 5 s).
        let hover_until = |app: &mut StrataApp, p: Pos2, want: &std::path::Path| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                frame_held(app, &ctx, vec![Event::PointerMoved(p)], link_mods());
                let got = app.terminal.active_terminal().unwrap().hovered_link().map(|l| l.0.to_owned());
                if got.as_deref() == Some(want) || std::time::Instant::now() > deadline {
                    return got;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        };
        let first = screen_pos(&app, "foo.rs:1:1", 1);
        assert_eq!(hover_until(&mut app, first, &dir.join("a/foo.rs")), Some(dir.join("a/foo.rs")));
        // Let the shell go on to `cd ../b`, the modifier held all along.
        app.terminal.active_terminal().unwrap().write("\r");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !app.terminal.active_terminal().unwrap().screen_text().contains("second foo.rs") && std::time::Instant::now() < deadline {
            frame_held(&mut app, &ctx, vec![Event::PointerMoved(first)], link_mods());
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let second = screen_pos(&app, "foo.rs:2:1", 1);
        assert_eq!(hover_until(&mut app, second, &dir.join("b/foo.rs")), Some(dir.join("b/foo.rs")), "the shell's new folder");
        link_click(&mut app, &ctx, second);
        assert_eq!(app.doc().path.clone(), Some(dir.join("b/foo.rs")));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn answers_expire_so_new_files_become_links() {
        let dir = std::env::temp_dir().join(format!("strata-term-later-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ctx = egui::Context::default();
        let mut app = StrataApp::new(None, None);
        frame(&mut app, &ctx, vec![]);
        let shell = ("/bin/sh".to_owned(), vec!["-c".to_owned(), "echo 'see later.rs:1:1'; sleep 30".to_owned()]);
        app.terminal.tabs.push(crate::terminal::Terminal::spawn(Some(ctx.clone()), &dir, Some(shell), 80, 12).unwrap());
        app.terminal.visible = true;
        wait_for(&mut app, &ctx, "later.rs");
        let p = screen_pos(&app, "later.rs:1:1", 1);
        hover(&mut app, &ctx, p);
        assert!(app.terminal.active_terminal().unwrap().hovered_link().is_none(), "not a file yet");
        std::fs::write(dir.join("later.rs"), "// now\n").unwrap();
        // The modifier stays held: the old answer (and the resolver's own file
        // check) expire after 2 s each.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(6);
        while app.terminal.active_terminal().unwrap().hovered_link().is_none() && std::time::Instant::now() < deadline {
            frame_held(&mut app, &ctx, vec![Event::PointerMoved(p)], link_mods());
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(app.terminal.active_terminal().unwrap().hovered_link().is_some_and(|l| l.0 == dir.join("later.rs")));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn link_lookups_never_touch_the_disk_on_the_ui_thread() {
        // A slow network folder or a removed drive mustn't freeze the editor:
        // the underline appears when a background check answers.
        let dir = std::env::temp_dir().join(format!("strata-term-bg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("bg_check.rs"), "// one\n// two\n").unwrap();
        let ctx = egui::Context::default();
        let mut app = StrataApp::new(Some(dir.clone()), None);
        frame(&mut app, &ctx, vec![]);
        let shell = ("/bin/sh".to_owned(), vec!["-c".to_owned(), "echo 'see bg_check.rs:2:1'; echo DONE; sleep 30".to_owned()]);
        app.terminal.tabs.push(crate::terminal::Terminal::spawn(Some(ctx.clone()), &dir, Some(shell), 80, 12).unwrap());
        app.terminal.visible = true;
        wait_for(&mut app, &ctx, "DONE");
        let p = screen_pos(&app, "bg_check.rs:2:1", 1);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while app.terminal.active_terminal().unwrap().hovered_link().is_none() && std::time::Instant::now() < deadline {
            frame_held(&mut app, &ctx, vec![Event::PointerMoved(p)], link_mods());
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let (path, line, _, _) = app.terminal.active_terminal().unwrap().hovered_link().expect("the link appears");
        assert_eq!((path, line), (dir.join("bg_check.rs").as_path(), 1));
        let on_ui = crate::terminal::Terminal::disk_checks_on(std::thread::current().id());
        assert!(on_ui.iter().all(|p| !p.starts_with(&dir)), "checked on the UI thread: {on_ui:?}");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn send_to_terminal_brackets_the_paste_when_asked() {
        // The program turns on bracketed paste, so the text arrives marked, then Enter.
        let (ctx, mut app) = shell_app("printf '\\033[?2004h'; cat -v");
        let file = std::env::temp_dir().join(format!("strata-send-{}.txt", std::process::id()));
        std::fs::write(&file, "a\nb\n").unwrap();
        app.open_path(file.clone(), false);
        app.doc().editor.select_ranges(&[0..4], 0);
        app.run(&ctx, Cmd::SendToTerminal);
        let screen = wait_for(&mut app, &ctx, "b^[[201~");
        assert!(screen.contains("^[[200~a\nb^[[201~\n"), "{screen}");
        std::fs::remove_file(file).ok();
    }

    #[test]
    fn send_to_terminal_runs_lines_in_a_real_shell() {
        // zsh's line editor turns on bracketed paste; plain sh doesn't, so its
        // lines are typed one by one. Either way each line runs.
        let shell = if std::path::Path::new("/bin/zsh").exists() { ("/bin/zsh".to_owned(), vec!["-f".to_owned()]) } else { ("/bin/sh".to_owned(), vec![]) };
        let ctx = egui::Context::default();
        let mut app = StrataApp::new(None, None);
        frame(&mut app, &ctx, vec![]);
        let file = std::env::temp_dir().join(format!("strata-send-shell-{}.sh", std::process::id()));
        std::fs::write(&file, "echo $((6*7))\necho one-$((1))\necho two-$((1+1))\n\necho $((2+3))X\n").unwrap();
        app.open_path(file.clone(), false);
        app.terminal.tabs.push(crate::terminal::Terminal::spawn(Some(ctx.clone()), &std::env::temp_dir(), Some(shell), 80, 20).unwrap());
        app.terminal.visible = true;
        let row = |screen: &str, text: &str| screen.lines().position(|l| l.trim() == text);
        // With nothing selected, the cursor's line; sent twice at once, while
        // the shell may still be starting, and run in order.
        app.doc().editor.select_ranges(&[0..0], 0);
        app.run(&ctx, Cmd::SendToTerminal);
        app.doc().editor.select_ranges(&[56..56], 0);
        app.run(&ctx, Cmd::SendToTerminal);
        let screen = wait_for(&mut app, &ctx, "5X");
        assert!(row(&screen, "42").is_some() && row(&screen, "42") < row(&screen, "5X"), "{screen}");
        // Two lines selected.
        app.doc().editor.select_ranges(&[14..48], 0);
        app.run(&ctx, Cmd::SendToTerminal);
        let screen = wait_for(&mut app, &ctx, "two-2");
        assert!(row(&screen, "one-1").is_some() && row(&screen, "two-2").is_some(), "{screen}");
        assert!(!app.terminal.has_focus(&ctx), "the editor keeps the keyboard");
        std::fs::remove_file(file).ok();
    }

    /// Whether `python3` can be run.
    fn have_python() -> bool {
        std::process::Command::new("python3").args(["-c", ""]).status().is_ok_and(|s| s.success())
    }

    #[test]
    fn send_to_terminal_closes_an_open_python_block() {
        if !have_python() {
            return;
        }
        let file = std::env::temp_dir().join(format!("strata-send-py-{}.py", std::process::id()));
        std::fs::write(&file, "for i in range(2):\n    print(i * 7)\n").unwrap();
        // The new REPL (bracketed paste) and the basic one (lines typed one by one).
        for basic in [false, true] {
            let env = if basic { "PYTHON_BASIC_REPL=1 " } else { "" };
            let shell = ("/bin/sh".to_owned(), vec!["-c".to_owned(), format!("{env}exec python3 -q")]);
            let ctx = egui::Context::default();
            let mut app = StrataApp::new(None, None);
            frame(&mut app, &ctx, vec![]);
            app.open_path(file.clone(), false);
            app.terminal.tabs.push(crate::terminal::Terminal::spawn(Some(ctx.clone()), &std::env::temp_dir(), Some(shell), 80, 20).unwrap());
            app.terminal.visible = true;
            wait_for(&mut app, &ctx, ">>>");
            app.doc().editor.select_all();
            app.run(&ctx, Cmd::SendToTerminal);
            let screen = wait_for(&mut app, &ctx, "\n7\n");
            let lines: Vec<&str> = screen.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
            assert!(lines.contains(&"0") && lines.contains(&"7"), "the loop ran (basic: {basic}): {screen}");
            // Back at the main prompt, not stuck at "...".
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            let mut last = String::new();
            while std::time::Instant::now() < deadline {
                let screen = wait_for(&mut app, &ctx, ">>>");
                last = screen.lines().map(str::trim).rfind(|l| !l.is_empty()).unwrap_or_default().to_owned();
                if last == ">>>" {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            assert_eq!(last, ">>>", "basic: {basic}");
        }
        std::fs::remove_file(file).ok();
    }

    #[test]
    fn an_indented_line_runs_once_in_a_shell() {
        // The extra Enter after an indented line is only an empty prompt to a
        // shell, and text sent right after it still runs, in order.
        let shell = if std::path::Path::new("/bin/zsh").exists() { ("/bin/zsh".to_owned(), vec!["-f".to_owned()]) } else { ("/bin/sh".to_owned(), vec![]) };
        let ctx = egui::Context::default();
        let mut app = StrataApp::new(None, None);
        frame(&mut app, &ctx, vec![]);
        let file = std::env::temp_dir().join(format!("strata-send-indent-{}.sh", std::process::id()));
        std::fs::write(&file, "if true; then\n  echo in-$((3*3))\nfi\necho after-$((2*2))\n").unwrap();
        app.open_path(file.clone(), false);
        app.terminal.tabs.push(crate::terminal::Terminal::spawn(Some(ctx.clone()), &std::env::temp_dir(), Some(shell), 80, 20).unwrap());
        app.terminal.visible = true;
        app.doc().editor.select_ranges(&[16..16], 0);
        app.run(&ctx, Cmd::SendToTerminal);
        app.doc().editor.select_ranges(&[38..38], 0);
        app.run(&ctx, Cmd::SendToTerminal);
        let screen = wait_for(&mut app, &ctx, "after-4");
        let row = |text: &str| screen.lines().position(|l| l.trim() == text);
        assert!(row("in-9").is_some() && row("in-9") < row("after-4"), "{screen}");
        assert_eq!(screen.matches("in-9").count(), 1, "ran once: {screen}");
        assert!(!screen.contains("not found") && !screen.contains("error"), "{screen}");
        std::fs::remove_file(file).ok();
    }

    #[test]
    fn send_to_terminal_opens_one_when_needed() {
        let ctx = egui::Context::default();
        let mut app = StrataApp::new(None, None);
        frame(&mut app, &ctx, vec![]);
        assert!(app.terminal.tabs.is_empty());
        // The default key, pressed in the editor (it isn't typed as a line break).
        let (mods, k) = Cmd::SendToTerminal.default_keys()[0];
        frame(&mut app, &ctx, vec![key(k, mods)]);
        assert!(app.terminal.visible && app.terminal.tabs.len() == 1);
        assert_eq!(app.docs[app.active].editor.buffer.len_lines(), 1, "nothing typed into the editor");
        app.run(&ctx, Cmd::SendToTerminal);
        assert_eq!(app.terminal.tabs.len(), 1, "later sends reuse it");
    }

    /// However a terminal is opened (palette, menu, shortcut, toggle), it
    /// gets the keyboard: what you type next goes to the shell, not the file.
    #[test]
    fn a_new_terminal_gets_the_keyboard() {
        let palette = if crate::keys::MAC { Modifiers::MAC_CMD | Modifiers::COMMAND | Modifiers::SHIFT } else { Modifiers::CTRL | Modifiers::COMMAND | Modifiers::SHIFT };
        type Open<'a> = &'a dyn Fn(&mut StrataApp, &egui::Context);
        let ways: [(&str, Open); 4] = [
            ("palette", &|app, ctx| {
                frame(app, ctx, vec![key(Key::P, palette)]);
                frame(app, ctx, vec![Event::Text("new terminal".into())]);
                frame(app, ctx, vec![]);
                assert_eq!(app.palette.selected_label(), Some("Terminal: New Terminal"));
                frame(app, ctx, vec![key(Key::Enter, Modifiers::NONE)]);
            }),
            ("menu", &|app, ctx| app.run_act(ctx, crate::menus::Act::Cmd(Cmd::NewTerminal))),
            ("shortcut", &|app, ctx| {
                let (mods, k) = Cmd::NewTerminal.default_keys()[0];
                frame(app, ctx, vec![key(k, mods)]);
            }),
            ("toggle", &|app, ctx| {
                let (mods, k) = Cmd::ToggleTerminal.default_keys()[0];
                frame(app, ctx, vec![key(k, mods)]);
            }),
        ];
        for (way, open) in ways {
            let ctx = egui::Context::default();
            let mut app = StrataApp::new(None, None);
            app.docs[0].editor.insert_text("file");
            frame(&mut app, &ctx, vec![]);
            app.doc().view.focus();
            frame(&mut app, &ctx, vec![]);
            assert!(app.docs[app.active].view.has_focus(&ctx));
            open(&mut app, &ctx);
            frame(&mut app, &ctx, vec![]);
            frame(&mut app, &ctx, vec![]);
            assert_eq!(app.terminal.tabs.len(), 1, "{way}");
            assert!(app.terminal.has_focus(&ctx), "{way}: the new terminal has the keyboard");
            frame(&mut app, &ctx, vec![Event::Text("x".into())]);
            assert_eq!(app.docs[app.active].editor.buffer.line_text(0), "file", "{way}: nothing typed into the file");
        }
    }

    #[test]
    fn toggle_type_and_hide() {
        let ctx = egui::Context::default();
        let mut app = StrataApp::new(None, None);
        frame(&mut app, &ctx, vec![]);
        frame(&mut app, &ctx, vec![key(Key::Backtick, Modifiers::CTRL)]);
        assert!(app.terminal.visible && app.terminal.tabs.len() == 1);
        frame(&mut app, &ctx, vec![]);
        assert!(app.terminal.has_focus(&ctx), "toggling on focuses the terminal");

        // Typed text goes to the shell, and Ctrl+G (Goto Line) stays in the terminal.
        let typed = vec![Event::Text("echo $((6*7))".into()), key(Key::Enter, Modifiers::NONE), key(Key::G, Modifiers::CTRL)];
        frame(&mut app, &ctx, typed);
        assert!(!app.palette.open, "the terminal owns Ctrl keys");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        let mut screen = String::new();
        while std::time::Instant::now() < deadline {
            frame(&mut app, &ctx, vec![]);
            screen = app.terminal.active_terminal().unwrap().screen_text();
            if screen.lines().any(|l| l.trim() == "42") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        assert!(screen.lines().any(|l| l.trim() == "42"), "{screen}");

        // Window shortcuts that aren't the terminal's still work.
        let palette = if crate::keys::MAC { Modifiers::MAC_CMD | Modifiers::COMMAND | Modifiers::SHIFT } else { Modifiers::CTRL | Modifiers::COMMAND | Modifiers::SHIFT };
        frame(&mut app, &ctx, vec![key(Key::P, palette)]);
        assert!(app.palette.open);
        app.palette.open = false;

        app.run(&ctx, Cmd::ToggleTerminal);
        assert!(!app.terminal.visible && app.terminal.tabs.len() == 1, "hiding keeps the shell");
        frame(&mut app, &ctx, vec![]);
        frame(&mut app, &ctx, vec![]);
        assert!(app.docs[app.active].view.has_focus(&ctx), "the editor gets the keyboard back");
        app.run(&ctx, Cmd::NewTerminal);
        assert_eq!(app.terminal.tabs.len(), 2);
    }
}
