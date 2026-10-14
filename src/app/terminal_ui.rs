//! The app's side of the terminal panel: toggling, new tabs, layout, and
//! which window shortcuts still work while a shell has the keyboard.

use super::StrataApp;
use crate::terminal::{self, PanelAction};

impl StrataApp {
    fn terminal_dir(&self) -> std::path::PathBuf {
        // The folder holding the current file, else the first folder.
        let file = self.docs[self.active].path.as_deref();
        let folder = file.and_then(|f| self.sidebar.root_of(f)).or(self.sidebar.root());
        terminal::start_dir(folder, file)
    }

    fn set_terminal_visible(&mut self, on: bool) {
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
        let scenes = [
            ("tabs", "printf 'A\\tB 中文\\nname\\tsize\\tdate\\nab\\tc\\td\\n中文\\tX\\tY\\n|1234567|1234567|1234567|\\n\\033[4mun\\tder\\033[0m|\\n'; ls -l / | head -4; echo DONE; sleep 30".to_owned(), "DONE"),
            ("emoji", "printf 'thumbs 👍🏽 |\\nfamily 👨\\342\\200\\215👩\\342\\200\\215👧 |\\nkeycap 1\\357\\270\\217\\342\\203\\243 |\\ncafe\\314\\201 |\\nheart ❤\\357\\270\\217 |\\nok 👍 |\\n'; echo DONE; sleep 30".to_owned(), "DONE"),
            ("less", format!("less {}; sleep 30", file.display()), "line 5"),
            ("ime", "printf 'typing: '; sleep 30".to_owned(), "typing"),
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
            let events = if name == "ime" { vec![Event::Ime(egui::ImeEvent::Preedit { text: "ni hao 你好".into(), active_range_chars: None })] } else { vec![] };
            sh.shoot(&ctx, &mut app, 3, events, &dir.join(format!("term_{name}.ppm")));
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
