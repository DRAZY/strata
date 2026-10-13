//! Color schemes in the app: which one shows (fixed, or following the
//! system's light/dark appearance), the user's color overrides, and the
//! grouped Color Scheme menus.

use super::StrataApp;
use crate::commands::Cmd;
use crate::highlight::scheme::{self, Slot};
use crate::highlight::{self, Adjustments};
use crate::menus::{Act, Item};
use std::sync::atomic::{AtomicU8, Ordering};

/// The system appearance eframe last reported: 0 unknown, 1 dark, 2 light.
static SYSTEM: AtomicU8 = AtomicU8::new(0);
/// The last unknown scheme name reported, so the status bar says it once.
static REPORTED: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

fn system_light() -> Option<bool> {
    match SYSTEM.load(Ordering::Relaxed) {
        1 => Some(false),
        2 => Some(true),
        _ => None,
    }
}

fn is_auto(mode: &str) -> bool {
    matches!(mode.trim().to_ascii_lowercase().as_str(), "auto" | "system")
}

impl StrataApp {
    /// The scheme the settings call for right now.
    pub(super) fn active_theme(&self) -> String {
        let s = &self.settings;
        scheme::choose(&s.theme_mode, &s.theme, &s.dark_theme, &s.light_theme, system_light()).to_owned()
    }

    fn adjustments(&self) -> Adjustments {
        let s = &self.settings;
        let opt = |name: &str| s.font_options.iter().any(|o| o.eq_ignore_ascii_case(name));
        Adjustments {
            overrides: s.theme_overrides.clone(),
            by_theme: s.theme_overrides_by_theme.clone(),
            no_italic: opt("no_italic"),
            no_underline: opt("no_underline"),
        }
    }

    /// Every frame: follow the system appearance and pick up changed color
    /// overrides. A scheme being previewed in the palette is left alone.
    pub(super) fn sync_appearance(&mut self, ctx: &egui::Context) {
        let system = match ctx.system_theme() {
            Some(egui::Theme::Dark) => 1,
            Some(egui::Theme::Light) => 2,
            None => 0,
        };
        SYSTEM.store(system, Ordering::Relaxed);
        let adj = self.adjustments();
        if highlight::set_adjustments(adj.clone()) {
            let problems = adj.problems();
            if !problems.is_empty() {
                self.status = format!("Settings: {}", problems.join("; "));
            }
        }
        if self.palette.open && self.palette.mode() == crate::palette::Mode::Theme {
            return;
        }
        let want = self.active_theme();
        if self.docs.iter().any(|d| !d.highlighter.is_showing(&want)) {
            let mut reported = REPORTED.lock().unwrap_or_else(|e| e.into_inner());
            if highlight::theme_canonical_name(&want).is_none() && *reported != want {
                self.status = format!("Unknown color scheme \"{want}\" (UI: Select Color Scheme lists them)");
                reported.clone_from(&want);
            }
            for d in &mut self.docs {
                d.highlighter.set_theme(&want);
            }
        }
    }

    /// A scheme chosen from a menu or the palette. In Auto mode it becomes
    /// the light or the dark scheme, by its own kind.
    pub(super) fn pick_theme(&mut self, name: String) {
        let light = highlight::theme_kind(&name).is_some_and(|k| k.is_light());
        let mode = self.settings.theme_mode.clone();
        match scheme::slot_for(&mode, light) {
            Slot::Dark => self.settings.dark_theme = name.clone(),
            Slot::Light => self.settings.light_theme = name.clone(),
            Slot::Theme => {
                // A light scheme while pinned to dark (or the reverse) ends the pin.
                if !is_auto(&mode) && matches!(mode.trim().to_ascii_lowercase().as_str(), "dark" | "light") {
                    self.settings.theme_mode = "fixed".into();
                }
                // Sublime's "theme": "auto" is replaced by a real name.
                self.settings.theme = name.clone();
            }
        }
        if is_auto(&mode) && system_light().is_some_and(|sys| sys != light) {
            let (kind, when) = if light { ("light", "light") } else { ("dark", "dark") };
            self.status = format!("{name} is now the {kind} scheme for Auto; it shows while the system is in {when} mode");
        }
        self.apply_settings();
        self.persist_settings();
    }

    /// Turn following the system appearance on or off. Turning it on keeps
    /// the current scheme for its kind.
    pub(super) fn set_follow_system(&mut self, on: bool) {
        let current = self.active_theme();
        if on {
            match highlight::theme_kind(&current).map(|k| k.is_light()) {
                Some(true) => self.settings.light_theme = current,
                Some(false) => self.settings.dark_theme = current,
                None => {}
            }
            self.settings.theme_mode = "auto".into();
        } else {
            self.settings.theme_mode = "fixed".into();
            self.settings.theme = current;
        }
        self.apply_settings();
        self.persist_settings();
    }

    /// Menu bar: Color Scheme ▸ Dark / Light / High Contrast / … and Auto.
    pub(super) fn theme_menu_items(&self) -> Vec<Item> {
        let current = self.active_theme();
        let auto = is_auto(&self.settings.theme_mode) || self.settings.theme.eq_ignore_ascii_case("auto");
        let mut items: Vec<Item> = highlight::theme_groups()
            .into_iter()
            .map(|(group, names)| {
                let entries = names.into_iter().map(|t| Item::new(t, Act::Theme(t.to_owned())).checked(current == t)).collect();
                Item::Sub(group.label().into(), entries)
            })
            .collect();
        items.extend([
            Item::Separator,
            Item::new("Follow System Appearance (Auto)", Act::Named(if auto { "appearance:fixed" } else { "appearance:auto" }.into())).checked(auto),
            Item::cmd("Select Color Scheme…", Cmd::SelectTheme),
        ]);
        items
    }

    /// Status bar: the same menu, drawn by egui.
    pub(super) fn theme_menu_ui(&mut self, ui: &mut egui::Ui) {
        let current = self.active_theme();
        let mut picked = None;
        let mut follow = None;
        let auto = is_auto(&self.settings.theme_mode);
        ui.menu_button("Theme", |ui| {
            for (group, names) in highlight::theme_groups() {
                ui.menu_button(group.label(), |ui| {
                    for name in names {
                        if ui.selectable_label(current == name, name).clicked() {
                            picked = Some(name.to_owned());
                            ui.close();
                        }
                    }
                });
            }
            ui.separator();
            let mut on = auto;
            if ui.checkbox(&mut on, "Follow system appearance").changed() {
                follow = Some(on);
                ui.close();
            }
        })
        .response
        .on_hover_text(format!("Color scheme: {current}"));
        if let Some(name) = picked {
            self.pick_theme(name);
        }
        if let Some(on) = follow {
            self.set_follow_system(on);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picking_in_auto_mode_fills_the_matching_slot() {
        let mut app = StrataApp::new(None, None);
        app.settings.theme_mode = "auto".into();
        app.pick_theme("Catppuccin Latte".into());
        app.pick_theme("Tokyo Night".into());
        assert_eq!((app.settings.light_theme.as_str(), app.settings.dark_theme.as_str()), ("Catppuccin Latte", "Tokyo Night"));
        SYSTEM.store(2, Ordering::Relaxed);
        assert_eq!(app.active_theme(), "Catppuccin Latte");
        SYSTEM.store(1, Ordering::Relaxed);
        assert_eq!(app.active_theme(), "Tokyo Night");
        app.set_follow_system(false);
        assert_eq!((app.settings.theme_mode.as_str(), app.settings.theme.as_str()), ("fixed", "Tokyo Night"));
        app.pick_theme("Nord".into());
        assert_eq!(app.settings.theme, "Nord");
        app.set_follow_system(true);
        assert_eq!(app.settings.dark_theme, "Nord", "turning Auto on keeps the current scheme");
        SYSTEM.store(0, Ordering::Relaxed);
    }

    #[test]
    fn palette_preview_restores_on_cancel_and_keeps_indentation() {
        use crate::palette::Event;
        let ctx = egui::Context::default();
        let mut app = StrataApp::new(None, None);
        app.settings.theme = "Nord".into();
        app.apply_settings();
        app.docs[0].editor.tab_width = 2;
        app.palette_events(&ctx, vec![Event::Theme { name: "Dracula".into(), preview: true }]);
        assert_eq!(app.docs[0].highlighter.theme_name(), "Dracula", "arrowing previews");
        app.palette_events(&ctx, vec![Event::Cancel]);
        assert_eq!(app.docs[0].highlighter.theme_name(), "Nord", "Esc restores the scheme");
        assert_eq!(app.docs[0].editor.tab_width, 2, "and leaves the file's indentation alone");
        app.palette_events(&ctx, vec![Event::Theme { name: "Gruvbox Light".into(), preview: false }]);
        assert_eq!((app.settings.theme.as_str(), app.docs[0].highlighter.theme_name()), ("Gruvbox Light", "Gruvbox Light"));
    }

    #[test]
    fn overrides_reach_open_editors() {
        let ctx = egui::Context::default();
        let mut app = StrataApp::new(None, None);
        app.settings.theme = "Dracula".into();
        // Per-scheme, so tests running alongside with other schemes see no change.
        let only_dracula = |k: &str, v: &str| std::collections::BTreeMap::from([("Dracula".to_owned(), std::collections::BTreeMap::from([(k.to_owned(), v.to_owned())]))]);
        app.settings.theme_overrides_by_theme = only_dracula("background", "#123456");
        app.sync_appearance(&ctx);
        let hl = &app.docs[0].highlighter;
        assert_eq!(hl.theme_name(), "Dracula");
        assert_eq!(hl.background(), egui::Color32::from_rgb(0x12, 0x34, 0x56));
        app.settings.theme_overrides_by_theme.clear();
        app.sync_appearance(&ctx);
        assert_eq!(app.docs[0].highlighter.background(), egui::Color32::from_rgb(0x28, 0x2a, 0x36));
        app.settings.theme_overrides_by_theme = only_dracula("bogus", "#fff");
        app.sync_appearance(&ctx);
        assert!(app.status.contains("bogus"), "{}", app.status);
        highlight::set_adjustments(Adjustments::default());
    }
}
