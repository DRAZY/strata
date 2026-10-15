//! The welcome card: on the very first launch (no settings, no session, and
//! no card shown before), a small card over the empty editor offers a color
//! scheme, a font size and the first places to go. Help ▸ Welcome shows it
//! again. It never blocks: typing goes to the editor, Esc or × closes it,
//! and opening a file puts it away.

use super::StrataApp;
use crate::commands::Cmd;
use crate::session::Settings;
use egui::{Color32, Key, RichText, Sense, vec2};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Written to the config folder once the card has been shown.
const MARKER: &str = "welcome-shown";

#[derive(Default)]
pub struct FirstRun {
    pub open: bool,
    /// Shown by itself at the first launch: it goes away when a file opens.
    automatic: bool,
}

fn marker(dir: &Path) -> PathBuf {
    dir.join(MARKER)
}

/// Strata has never run with this config folder: no settings, no session,
/// and the card hasn't been shown.
pub fn is_first_run(dir: &Path) -> bool {
    !Settings::path(dir).exists() && !dir.join("session.json").exists() && !marker(dir).exists()
}

/// Remember the card was shown (with the version, should a later one want
/// to say what's new).
pub fn mark_shown(dir: &Path) {
    if crate::private::create_dir_all(dir).is_ok()
        && let Err(e) = std::fs::write(marker(dir), format!("{}\n", env!("CARGO_PKG_VERSION")))
    {
        log::warn!("couldn't write {}: {e}", marker(dir).display());
    }
}

/// What the card asks for, carried out after drawing.
enum Pick {
    Scheme(&'static str),
    FontSize(f32),
    UiSize(f32),
    Cmd(Cmd),
    Close,
}

/// The three schemes offered: (label, theme_mode, scheme shown).
const SCHEMES: [(&str, &str, &str); 3] = [("Dark", "fixed", "Strata Dark"), ("Light", "fixed", "Strata Light"), ("Auto", "auto", "")];

impl StrataApp {
    /// At launch: show the card if this is the first run. Tests and dev
    /// screenshots opt in by setting `first_run.open` themselves.
    pub(super) fn first_run_check(&mut self) {
        if cfg!(test) || crate::dev::var("STRATA_SCREENSHOT").is_some() {
            return;
        }
        if let Some(dir) = self.config.clone()
            && is_first_run(&dir)
        {
            self.show_welcome(true);
            mark_shown(&dir);
        }
    }

    /// Help ▸ Welcome (or the first launch, `automatic`).
    pub(super) fn show_welcome(&mut self, automatic: bool) {
        self.first_run = FirstRun { open: true, automatic };
    }

    /// Esc closes the card, when nothing else wants it.
    pub(super) fn first_run_keys(&mut self, ctx: &egui::Context) {
        if self.first_run.open && !self.palette.open && !self.find.open && !self.help.active && !self.settings_ui.active && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape)) {
            self.first_run.open = false;
        }
    }

    /// The card, centred over the editor area.
    pub(super) fn first_run_card(&mut self, ctx: &egui::Context) {
        if !self.first_run.open {
            return;
        }
        if self.first_run.automatic && !(self.docs.len() == 1 && self.docs[0].is_blank()) {
            self.first_run.open = false;
            return;
        }
        let area = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("panes-rect", self.link.id("panes"))))).unwrap_or_else(|| ctx.content_rect());
        let hl = &self.docs[self.active].highlighter;
        let (bg, accent) = (hl.background(), hl.accent());
        let size = self.ui_size();
        let width = (size * 34.0).clamp(380.0, 620.0).min(area.width() - 24.0);
        let s = &self.settings;
        let mode = s.theme_mode.trim().to_ascii_lowercase();
        let current = if matches!(mode.as_str(), "auto" | "system") { "Auto" } else if self.active_theme() == "Strata Light" { "Light" } else if self.active_theme() == "Strata Dark" { "Dark" } else { "" };
        let font_size = s.font_size;
        let mut pick = None;
        let frame = egui::Frame::window(&ctx.global_style()).fill(egui::lerp(egui::Rgba::from(bg)..=egui::Rgba::from(ctx.global_style().visuals.panel_fill), 0.5).into()).inner_margin(egui::Margin::same(20)).corner_radius(10);
        egui::Area::new(self.link.id("welcome-card"))
            .order(egui::Order::Middle)
            .pivot(egui::Align2::CENTER_CENTER)
            .fixed_pos(area.center())
            .constrain_to(area)
            .show(ctx, |ui| {
                frame.show(ui, |ui| {
                    ui.set_width(width);
                    let dim = ui.visuals().weak_text_color();
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Welcome to Strata").size(size * 1.7).strong());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.add(egui::Button::new(RichText::new("×").size(size * 1.4)).frame(false)).on_hover_text("Close (Esc)").clicked() {
                                pick = Some(Pick::Close);
                            }
                        });
                    });
                    let settings_key = Cmd::SettingsWindow.shortcut_label().map_or_else(String::new, |k| format!(" ({k})"));
                    ui.label(RichText::new(format!("A couple of choices to start. You can change them, and everything else, in Settings{settings_key}.")).size(size).color(dim));
                    ui.add_space(14.0);

                    ui.label(RichText::new("Color scheme").size(size).strong());
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        let tile = vec2(((width - 24.0) / 3.0).min((size * 11.0).max(150.0)), 74.0);
                        for (label, _, _) in SCHEMES {
                            ui.vertical(|ui| {
                                let r = scheme_tile(ui, label, tile, label == current, accent);
                                if r.clicked() {
                                    pick = Some(Pick::Scheme(label));
                                }
                                ui.allocate_ui_with_layout(vec2(tile.x, size + 6.0), egui::Layout::top_down(egui::Align::Center), |ui| {
                                    ui.set_width(tile.x);
                                    ui.label(RichText::new(if label == "Auto" { "Auto (follow system)" } else { label }).size(size));
                                });
                            });
                        }
                    });
                    ui.add_space(10.0);

                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Font size").size(size).strong());
                        ui.add_space(8.0);
                        if ui.add_enabled(font_size > 8.0, egui::Button::new(RichText::new("−").size(size))).on_hover_text("Smaller").clicked() {
                            pick = Some(Pick::FontSize(font_size - 1.0));
                        }
                        ui.label(RichText::new(format!("{} pt", crate::settings_meta::fmt_number(font_size as f64))).size(size).monospace());
                        if ui.add_enabled(font_size < 48.0, egui::Button::new(RichText::new("+").size(size))).on_hover_text("Larger").clicked() {
                            pick = Some(Pick::FontSize(font_size + 1.0));
                        }
                    });
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("Interface size").size(size).strong());
                        ui.add_space(8.0);
                        for (name, pt) in crate::fonts::UI_SIZE_PRESETS {
                            if ui.add(egui::Button::selectable(size == pt, RichText::new(name).size(size))).on_hover_text(format!("Menus, panels and dialogs at {pt} pt")).clicked() {
                                pick = Some(Pick::UiSize(pt));
                            }
                        }
                    });
                    ui.add_space(14.0);

                    ui.horizontal_wrapped(|ui| {
                        let mut button = |ui: &mut egui::Ui, label: &str, cmd: Cmd| {
                            let tip = cmd.shortcut_label().unwrap_or_default();
                            let r = ui.button(RichText::new(label).size(size));
                            if (if tip.is_empty() { r } else { r.on_hover_text(tip) }).clicked() {
                                pick = Some(Pick::Cmd(cmd));
                            }
                        };
                        button(ui, "Open a Folder…", Cmd::OpenFolder);
                        if cfg!(target_os = "macos") {
                            button(ui, "Install Shell Command", Cmd::InstallShellCommand);
                        }
                        button(ui, "All Settings…", Cmd::SettingsWindow);
                    });
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        let mut link = |ui: &mut egui::Ui, label: String, cmd: Cmd| {
                            if ui.link(RichText::new(label).size(size)).clicked() {
                                pick = Some(Pick::Cmd(cmd));
                            }
                        };
                        let manual = Cmd::Documentation.shortcut_label().map_or_else(|| "Read the manual".to_owned(), |k| format!("Read the manual ({k})"));
                        link(ui, manual, Cmd::GettingStarted);
                        ui.label(RichText::new("·").color(dim));
                        link(ui, "Keyboard shortcuts".into(), Cmd::KeyboardShortcuts);
                        ui.label(RichText::new("·").color(dim));
                        let palette = Cmd::CommandPalette.shortcut_label().map_or_else(|| "Command palette".to_owned(), |k| format!("Command palette ({k})"));
                        link(ui, palette, Cmd::CommandPalette);
                    });
                    ui.add_space(4.0);
                    ui.label(RichText::new("Help ▸ Welcome shows this again.").size(crate::fonts::secondary_size(size)).color(dim));
                });
            });
        match pick {
            Some(Pick::Scheme(label)) => self.welcome_scheme(ctx, label),
            Some(Pick::FontSize(v)) => self.change_setting(ctx, "font_size", Value::from(v as f64), true),
            Some(Pick::UiSize(v)) => self.change_setting(ctx, "ui_font_size", Value::from(v as f64), true),
            Some(Pick::Cmd(cmd)) => {
                if matches!(cmd, Cmd::OpenFolder | Cmd::SettingsWindow | Cmd::GettingStarted | Cmd::KeyboardShortcuts | Cmd::CommandPalette) {
                    self.first_run.open = false;
                }
                self.run(ctx, cmd);
            }
            Some(Pick::Close) => self.first_run.open = false,
            None => {}
        }
    }

    /// Dark, Light or Auto from the card.
    pub(super) fn welcome_scheme(&mut self, ctx: &egui::Context, label: &str) {
        let Some((_, mode, name)) = SCHEMES.iter().find(|(l, _, _)| *l == label) else { return };
        if !name.is_empty() {
            self.change_setting(ctx, "theme", Value::from(*name), false);
        }
        self.change_setting(ctx, "theme_mode", Value::from(*mode), true);
    }
}

/// A clickable preview of a scheme: Auto shows dark and light side by side.
fn scheme_tile(ui: &mut egui::Ui, label: &str, size: egui::Vec2, selected: bool, accent: Color32) -> egui::Response {
    let (rect, r) = ui.allocate_exact_size(size, Sense::click());
    let painter = ui.painter_at(rect.expand(3.0));
    let radius = 6.0;
    let draw = |area: egui::Rect, name: &str, corners: egui::CornerRadius| {
        let Some(p) = super::settings_ui::preview(name) else { return };
        painter.rect_filled(area, corners, p.bg);
        // A few lines of "code".
        let line = |y: f32, parts: &[(f32, Color32)]| {
            let mut x = area.left() + 10.0;
            for (w, c) in parts {
                let w = w * (area.width() - 20.0);
                painter.rect_filled(egui::Rect::from_min_size(egui::pos2(x, y), vec2((w - 4.0).max(2.0), 5.0)), 2.0, *c);
                x += w;
            }
        };
        let t = area.top() + 12.0;
        line(t, &[(0.22, p.syntax[0]), (0.38, p.fg), (0.3, p.syntax[1])]);
        line(t + 14.0, &[(0.12, p.bg), (0.5, p.fg), (0.2, p.syntax[0])]);
        line(t + 28.0, &[(0.12, p.bg), (0.6, p.syntax[2])]);
        line(t + 42.0, &[(0.3, p.fg)]);
    };
    let all = egui::CornerRadius::same(radius as u8);
    match label {
        "Dark" => draw(rect, "Strata Dark", all),
        "Light" => draw(rect, "Strata Light", all),
        _ => {
            let (l, rr) = rect.split_left_right_at_fraction(0.5);
            draw(l, "Strata Dark", egui::CornerRadius { ne: 0, se: 0, ..all });
            draw(rr, "Strata Light", egui::CornerRadius { nw: 0, sw: 0, ..all });
        }
    }
    let stroke = if selected {
        egui::Stroke::new(2.5, accent)
    } else if r.hovered() || r.has_focus() {
        egui::Stroke::new(1.5, ui.visuals().text_color().gamma_multiply(0.6))
    } else {
        ui.visuals().widgets.noninteractive.bg_stroke
    };
    ui.painter().rect_stroke(rect, radius, stroke, egui::StrokeKind::Outside);
    r.on_hover_text(match label {
        "Dark" => "Strata Dark",
        "Light" => "Strata Light",
        _ => "Strata Dark or Strata Light, following the system's appearance",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("strata-first-run-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn first_run_is_an_empty_config_folder_once() {
        let dir = temp_dir("detect");
        assert!(is_first_run(&dir));
        mark_shown(&dir);
        assert!(!is_first_run(&dir), "shown once");
        assert!(std::fs::read_to_string(dir.join(MARKER)).unwrap().starts_with(env!("CARGO_PKG_VERSION")));
        // Someone who used Strata before this card existed never sees it.
        let dir = temp_dir("settings");
        std::fs::write(dir.join("settings.json"), "{}").unwrap();
        assert!(!is_first_run(&dir));
        let dir = temp_dir("session");
        std::fs::write(dir.join("session.json"), "{}").unwrap();
        assert!(!is_first_run(&dir));
        // A folder that doesn't exist yet is a first run too.
        assert!(is_first_run(&dir.join("not-yet")));
        // Tests never see it by themselves, even with a fresh config folder.
        let dir = temp_dir("app");
        let app = StrataApp::new(None, Some(dir.clone()));
        assert!(!app.first_run.open);
        std::fs::remove_dir_all(dir).ok();
    }

    fn frame(app: &mut StrataApp, ctx: &egui::Context, events: Vec<egui::Event>) {
        let raw = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(1200.0, 800.0))), events, ..Default::default() };
        let mut out = ctx.run_ui(raw, |ui| eframe::App::ui(app, ui, &mut eframe::Frame::_new_kittest()));
        out.textures_delta.clear();
    }

    #[test]
    fn card_picks_apply_and_esc_closes() {
        let ctx = egui::Context::default();
        let mut app = StrataApp::new(None, None);
        app.show_welcome(true);
        app.welcome_scheme(&ctx, "Light");
        assert_eq!((app.settings.theme_mode.as_str(), app.settings.theme.as_str()), ("fixed", "Strata Light"));
        app.welcome_scheme(&ctx, "Auto");
        assert_eq!(app.settings.theme_mode, "auto");
        app.welcome_scheme(&ctx, "Dark");
        assert_eq!((app.settings.theme_mode.as_str(), app.active_theme().as_str()), ("fixed", "Strata Dark"));
        let esc = egui::Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE };
        frame(&mut app, &ctx, vec![]);
        assert!(app.first_run.open);
        frame(&mut app, &ctx, vec![esc]);
        assert!(!app.first_run.open, "Esc closes it");
        // The first-run card goes away once a file is open; Help ▸ Welcome's stays.
        app.show_welcome(true);
        app.doc().editor.insert_text("hello");
        frame(&mut app, &ctx, vec![]);
        assert!(!app.first_run.open);
        app.run(&ctx, Cmd::Welcome);
        frame(&mut app, &ctx, vec![]);
        assert!(app.first_run.open);
    }
}
