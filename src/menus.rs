//! The menu bar. One description (built by the app from its current state)
//! renders as native menus on macOS and as an in-window menu bar elsewhere,
//! the way Sublime does on each platform.

use crate::commands::Cmd;
use egui::{Key, Modifiers};
use std::path::PathBuf;

/// What choosing a menu item does.
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    Cmd(Cmd),
    /// An editor command by id (see the app's EDITOR_COMMANDS).
    Editor(&'static str),
    /// Clipboard and undo go to whichever widget has focus (editor or a text field).
    Copy,
    Cut,
    Paste,
    Undo,
    Redo,
    SelectAll,
    OpenRecent(PathBuf),
    OpenRecentFolder(PathBuf),
    Syntax(String),
    Theme(String),
    TabWidth(usize),
    Tab(usize),
    Quit,
    ReopenWithEncoding(crate::encoding::Encoding),
    SaveWithEncoding(crate::encoding::Encoding),
    /// Anything the app defines with a string id (encodings, build variants…).
    Named(String),
}

pub enum Item {
    Action { label: String, act: Act, keys: Option<(Modifiers, Key)>, hint: Option<String>, check: Option<bool>, enabled: bool },
    Separator,
    Sub(String, Vec<Item>),
    /// macOS-only system items; skipped elsewhere.
    System(#[cfg_attr(not(target_os = "macos"), allow(dead_code))] System),
}

#[derive(Clone, Copy)]
pub enum System {
    Services,
    Hide,
    HideOthers,
    ShowAll,
    Minimize,
    Zoom,
    BringAllToFront,
}

impl Item {
    pub fn new(label: impl Into<String>, act: Act) -> Self {
        Item::Action { label: label.into(), act, keys: None, hint: None, check: None, enabled: true }
    }

    /// A window command, with its shortcut from the command table.
    pub fn cmd(label: impl Into<String>, cmd: Cmd) -> Self {
        Item::new(label, Act::Cmd(cmd)).keys(cmd.keys().first().copied())
    }

    pub fn keys(mut self, k: Option<(Modifiers, Key)>) -> Self {
        if let Item::Action { keys, .. } = &mut self {
            *keys = k;
        }
        self
    }

    pub fn key(self, mods: Modifiers, key: Key) -> Self {
        self.keys(Some((mods, key)))
    }

    /// Shortcut text for chords the menu can't bind ("⌘K ⌘U").
    pub fn hint(mut self, text: &str) -> Self {
        if let Item::Action { hint, .. } = &mut self {
            *hint = Some(crate::keys::spaced(text));
        }
        self
    }

    pub fn checked(mut self, on: bool) -> Self {
        if let Item::Action { check, .. } = &mut self {
            *check = Some(on);
        }
        self
    }

    pub fn enabled(mut self, on: bool) -> Self {
        if let Item::Action { enabled, .. } = &mut self {
            *enabled = on;
        }
        self
    }
}

pub type Menus = Vec<(String, Vec<Item>)>;

/// A cheap fingerprint of a menu description, to rebuild native menus only on change.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn fingerprint(menus: &Menus) -> u64 {
    use std::hash::{Hash, Hasher};
    fn walk(items: &[Item], h: &mut std::collections::hash_map::DefaultHasher) {
        for it in items {
            match it {
                Item::Action { label, check, enabled, hint, .. } => (label, check, enabled, hint).hash(h),
                Item::Separator => 0u8.hash(h),
                Item::Sub(l, kids) => {
                    l.hash(h);
                    walk(kids, h);
                }
                Item::System(_) => 1u8.hash(h),
            }
        }
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for (name, items) in menus {
        name.hash(&mut h);
        walk(items, &mut h);
    }
    h.finish()
}

// ---- in-window menu bar (Windows, Linux) ------------------------------------

/// Draw the menus as an egui menu bar; returns the chosen action.
pub fn menu_bar(ui: &mut egui::Ui, menus: &Menus) -> Option<Act> {
    let mut chosen = None;
    egui::MenuBar::new().ui(ui, |ui| {
        for (name, items) in menus {
            ui.menu_button(name, |ui| items_ui(ui, items, &mut chosen));
        }
    });
    chosen
}

fn items_ui(ui: &mut egui::Ui, items: &[Item], chosen: &mut Option<Act>) {
    for it in items {
        match it {
            Item::Action { label, act, keys, hint, check, enabled } => {
                let shortcut = hint.clone().or_else(|| keys.map(|(m, k)| crate::keys::label(m, k))).unwrap_or_default();
                let text = match check {
                    Some(true) => format!("✔ {label}"),
                    Some(false) => format!("    {label}"),
                    None => label.clone(),
                };
                let button = egui::Button::new(text).shortcut_text(shortcut);
                if ui.add_enabled(*enabled, button).clicked() {
                    *chosen = Some(act.clone());
                    ui.close();
                }
            }
            Item::Separator => {
                ui.separator();
            }
            Item::Sub(name, kids) => {
                ui.menu_button(name, |ui| {
                    egui::ScrollArea::vertical().max_height(560.0).show(ui, |ui| items_ui(ui, kids, chosen));
                });
            }
            Item::System(_) => {}
        }
    }
}

// ---- native menus (macOS) ----------------------------------------------------

#[cfg(target_os = "macos")]
pub mod native {
    use super::{Act, Item, Menus, System};
    use egui::{Key, Modifiers};
    use muda::accelerator::{Accelerator, Code, Modifiers as M};
    use muda::{CheckMenuItem, IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
    use std::collections::HashMap;
    use std::sync::mpsc::{Receiver, channel};

    pub struct NativeMenu {
        menu: Option<Menu>,
        actions: HashMap<muda::MenuId, Act>,
        rx: Receiver<muda::MenuId>,
        built: u64,
    }

    impl NativeMenu {
        /// Menu clicks wake the app so they're handled right away.
        pub fn new(ctx: &egui::Context) -> Self {
            let (tx, rx) = channel();
            let ctx = ctx.clone();
            MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
                let _ = tx.send(e.id);
                ctx.request_repaint();
            }));
            Self { menu: None, actions: HashMap::new(), rx, built: 0 }
        }

        /// Install `menus` as the app's menu bar if they changed since last time.
        pub fn update(&mut self, menus: &Menus) {
            let fp = super::fingerprint(menus);
            if self.menu.is_some() && fp == self.built {
                return;
            }
            self.built = fp;
            self.actions.clear();
            let menu = Menu::new();
            for (name, items) in menus {
                let sub = Submenu::new(name, true);
                self.fill(&sub, items);
                let _ = menu.append(&sub);
                if name == "Window" {
                    sub.set_as_windows_menu_for_nsapp();
                } else if name == "Help" {
                    sub.set_as_help_menu_for_nsapp();
                }
            }
            menu.init_for_nsapp();
            self.menu = Some(menu);
        }

        fn fill(&mut self, sub: &Submenu, items: &[Item]) {
            for it in items {
                let item: Box<dyn IsMenuItem> = match it {
                    Item::Action { label, act, keys, hint, check, enabled } => {
                        // Chords can't be accelerators; show them in the title instead.
                        let text = match hint {
                            Some(h) => format!("{label}      {h}"),
                            None => label.clone(),
                        };
                        let accel = keys.and_then(|(m, k)| accelerator(m, k));
                        let id = match check {
                            Some(on) => {
                                let c = CheckMenuItem::new(&text, *enabled, *on, accel);
                                let id = c.id().clone();
                                let _ = sub.append(&c);
                                id
                            }
                            None => {
                                let m = MenuItem::new(&text, *enabled, accel);
                                let id = m.id().clone();
                                let _ = sub.append(&m);
                                id
                            }
                        };
                        self.actions.insert(id, act.clone());
                        continue;
                    }
                    Item::Separator => Box::new(PredefinedMenuItem::separator()),
                    Item::Sub(name, kids) => {
                        let s = Submenu::new(name, true);
                        self.fill(&s, kids);
                        Box::new(s)
                    }
                    Item::System(sys) => Box::new(match sys {
                        System::Services => PredefinedMenuItem::services(None),
                        System::Hide => PredefinedMenuItem::hide(Some("Hide Strata")),
                        System::HideOthers => PredefinedMenuItem::hide_others(None),
                        System::ShowAll => PredefinedMenuItem::show_all(None),
                        System::Minimize => PredefinedMenuItem::minimize(None),
                        System::Zoom => PredefinedMenuItem::maximize(Some("Zoom")),
                        System::BringAllToFront => PredefinedMenuItem::bring_all_to_front(None),
                    }),
                };
                let _ = sub.append(item.as_ref());
            }
        }

        /// Actions chosen since the last call.
        pub fn poll(&self) -> Vec<Act> {
            self.rx.try_iter().filter_map(|id| self.actions.get(&id).cloned()).collect()
        }
    }

    /// Menu shortcuts. Unmodified keys (other than F-keys) are never bound,
    /// so the menu can't steal Escape or Enter from text fields.
    fn accelerator(mods: Modifiers, key: Key) -> Option<Accelerator> {
        let mut m = M::empty();
        if mods.mac_cmd || mods.command {
            m |= M::META;
        }
        if mods.ctrl {
            m |= M::CONTROL;
        }
        if mods.alt {
            m |= M::ALT;
        }
        if mods.shift {
            m |= M::SHIFT;
        }
        let code = code(key)?;
        let fkey = format!("{code:?}").starts_with('F') && code != Code::KeyF;
        if m.is_empty() && !fkey {
            return None;
        }
        Some(Accelerator::new(m, code))
    }

    fn code(key: Key) -> Option<Code> {
        use Key as K;
        Some(match key {
            K::A => Code::KeyA, K::B => Code::KeyB, K::C => Code::KeyC, K::D => Code::KeyD, K::E => Code::KeyE,
            K::F => Code::KeyF, K::G => Code::KeyG, K::H => Code::KeyH, K::I => Code::KeyI, K::J => Code::KeyJ,
            K::K => Code::KeyK, K::L => Code::KeyL, K::M => Code::KeyM, K::N => Code::KeyN, K::O => Code::KeyO,
            K::P => Code::KeyP, K::Q => Code::KeyQ, K::R => Code::KeyR, K::S => Code::KeyS, K::T => Code::KeyT,
            K::U => Code::KeyU, K::V => Code::KeyV, K::W => Code::KeyW, K::X => Code::KeyX, K::Y => Code::KeyY,
            K::Z => Code::KeyZ,
            K::Num0 => Code::Digit0, K::Num1 => Code::Digit1, K::Num2 => Code::Digit2, K::Num3 => Code::Digit3,
            K::Num4 => Code::Digit4, K::Num5 => Code::Digit5, K::Num6 => Code::Digit6, K::Num7 => Code::Digit7,
            K::Num8 => Code::Digit8, K::Num9 => Code::Digit9,
            K::Slash => Code::Slash, K::Backslash => Code::Backslash, K::OpenBracket => Code::BracketLeft,
            K::CloseBracket => Code::BracketRight, K::Comma => Code::Comma, K::Period => Code::Period,
            K::Equals => Code::Equal, K::Minus => Code::Minus, K::Semicolon => Code::Semicolon,
            K::Backtick => Code::Backquote, K::Quote => Code::Quote,
            K::ArrowUp => Code::ArrowUp, K::ArrowDown => Code::ArrowDown, K::ArrowLeft => Code::ArrowLeft,
            K::ArrowRight => Code::ArrowRight, K::Enter => Code::Enter, K::Tab => Code::Tab, K::Space => Code::Space,
            K::Backspace => Code::Backspace, K::Delete => Code::Delete, K::Home => Code::Home, K::End => Code::End,
            K::PageUp => Code::PageUp, K::PageDown => Code::PageDown,
            K::F1 => Code::F1, K::F2 => Code::F2, K::F3 => Code::F3, K::F4 => Code::F4, K::F5 => Code::F5,
            K::F6 => Code::F6, K::F7 => Code::F7, K::F8 => Code::F8, K::F9 => Code::F9, K::F10 => Code::F10,
            K::F11 => Code::F11, K::F12 => Code::F12,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_tracks_checks_and_labels() {
        let a: Menus = vec![("View".into(), vec![Item::new("Word Wrap", Act::Cmd(Cmd::ToggleWordWrap)).checked(false)])];
        let b: Menus = vec![("View".into(), vec![Item::new("Word Wrap", Act::Cmd(Cmd::ToggleWordWrap)).checked(true)])];
        assert_ne!(fingerprint(&a), fingerprint(&b));
        assert_eq!(fingerprint(&a), fingerprint(&a));
    }
}
