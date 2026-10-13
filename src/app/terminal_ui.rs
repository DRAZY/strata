//! The app's side of the terminal panel: toggling, new tabs, layout, and
//! which window shortcuts still work while a shell has the keyboard.

use super::StrataApp;
use crate::terminal::{self, PanelAction};

impl StrataApp {
    fn terminal_dir(&self) -> std::path::PathBuf {
        terminal::start_dir(self.sidebar.root.as_deref(), self.docs[self.active].path.as_deref())
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
            Err(e) => self.status = e,
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
            let dev = std::env::var("STRATA_DEV_TERMINAL").ok();
            if self.settings.terminal_visible || dev.is_some() {
                self.new_terminal(&ctx);
                if let (Some(cmd), Some(t)) = (dev, self.terminal.active_terminal()) {
                    t.write(format!("{cmd}\r"));
                    self.doc().view.cancel_focus();
                }
            }
        }
        if self.terminal.poll(&ctx) {
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

    fn frame(app: &mut StrataApp, ctx: &egui::Context, events: Vec<Event>) {
        let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 700.0))), events, ..Default::default() };
        let mut out = ctx.run_ui(raw, |ui| {
            let mut frame = eframe::Frame::_new_kittest();
            eframe::App::ui(app, ui, &mut frame);
        });
        out.textures_delta.clear();
    }

    fn key(key: Key, modifiers: Modifiers) -> Event {
        Event::Key { key, physical_key: Some(key), pressed: true, repeat: false, modifiers }
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
