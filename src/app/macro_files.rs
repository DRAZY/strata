//! Saved macros: Tools ▸ Save Macro… writes the last recorded macro to
//! `<config>/Macros/<name>.strata-macro` (JSON), Tools ▸ Macros runs one,
//! and keybindings.json binds one with
//! `{"command": "run_macro_file", "args": {"file": "<name>"}}`.

use super::StrataApp;
use crate::keybindings::Target;
use crate::menus::{Act, Item};
use egui::{Event, Key, Modifiers};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const EXTENSION: &str = "strata-macro";
/// Menu action prefix: "macro:<name>".
pub(super) const RUN_PREFIX: &str = "macro:";

/// A macro file: the version and the recorded input.
#[derive(Serialize, Deserialize, Default, Debug, PartialEq)]
#[serde(default)]
pub struct MacroFile {
    pub strata_macro: u32,
    pub steps: Vec<Step>,
}

/// One recorded input: typed text, a paste, or a key with its modifiers.
#[derive(Serialize, Deserialize, Default, Debug, PartialEq, Clone)]
#[serde(default)]
pub struct Step {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paste: Option<String>,
    /// egui's key name ("Enter", "ArrowLeft", "A").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// "alt", "ctrl", "shift", "mac_cmd", "command".
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub mods: Vec<String>,
}

const MODS: [&str; 5] = ["alt", "ctrl", "shift", "mac_cmd", "command"];

fn mod_flags(m: &mut Modifiers) -> [&mut bool; 5] {
    [&mut m.alt, &mut m.ctrl, &mut m.shift, &mut m.mac_cmd, &mut m.command]
}

impl Step {
    fn from_event(e: &Event) -> Option<Step> {
        match e {
            Event::Text(t) => Some(Step { text: Some(t.clone()), ..Default::default() }),
            Event::Paste(t) => Some(Step { paste: Some(t.clone()), ..Default::default() }),
            Event::Key { key, pressed: true, modifiers, .. } => {
                let mut m = *modifiers;
                let mods = MODS.iter().zip(mod_flags(&mut m)).filter(|(_, on)| **on).map(|(n, _)| (*n).to_owned()).collect();
                Some(Step { key: Some(key.name().to_owned()), mods, ..Default::default() })
            }
            _ => None,
        }
    }

    fn to_event(&self) -> Result<Event, String> {
        if let Some(t) = &self.text {
            return Ok(Event::Text(t.clone()));
        }
        if let Some(t) = &self.paste {
            return Ok(Event::Paste(t.clone()));
        }
        let name = self.key.as_deref().ok_or("a step with no text, paste or key")?;
        let key = Key::from_name(name).ok_or_else(|| format!("unknown key \"{name}\""))?;
        let mut modifiers = Modifiers::NONE;
        for m in &self.mods {
            let i = MODS.iter().position(|n| n == m).ok_or_else(|| format!("unknown modifier \"{m}\""))?;
            for (k, flag) in mod_flags(&mut modifiers).into_iter().enumerate() {
                *flag |= k == i;
            }
        }
        Ok(Event::Key { key, physical_key: Some(key), pressed: true, repeat: false, modifiers })
    }
}

impl MacroFile {
    pub fn from_events(events: &[Event]) -> Self {
        MacroFile { strata_macro: 1, steps: events.iter().filter_map(Step::from_event).collect() }
    }

    pub fn events(&self) -> Result<Vec<Event>, String> {
        self.steps.iter().map(Step::to_event).collect()
    }
}

pub fn macros_dir(config: &Path) -> PathBuf {
    config.join("Macros")
}

/// Saved macros' names, sorted.
pub fn saved_macros(config: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(macros_dir(config))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            Some(name.strip_suffix(".strata-macro")?.to_owned())
        })
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names
}

/// Write `events` as JSON to `path`.
pub fn write_macro(path: &Path, events: &[Event]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string_pretty(&MacroFile::from_events(events)).map_err(std::io::Error::other)?;
    std::fs::write(path, json + "\n")
}

/// The saved macro `name`'s input.
pub fn read_macro(config: &Path, name: &str) -> Result<Vec<Event>, String> {
    let path = macros_dir(config).join(format!("{name}.{EXTENSION}"));
    let text = std::fs::read_to_string(&path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => format!("No saved macro named “{name}” (Tools ▸ Save Macro… saves one)"),
        _ => format!("Couldn't read the macro “{name}”: {e}"),
    })?;
    let file: MacroFile = serde_json::from_str(&crate::session::strip_jsonc(&text)).map_err(|e| format!("Macro “{name}”: {e}"))?;
    file.events().map_err(|e| format!("Macro “{name}”: {e}"))
}

impl StrataApp {
    /// The Tools ▸ Macros submenu: each saved macro, with its key if bound.
    pub(super) fn saved_macro_items(&self) -> Vec<Item> {
        let saved = self.config.as_deref().map(saved_macros).unwrap_or_default();
        let mut runs: Vec<Item> = saved
            .iter()
            .map(|n| {
                let item = Item::new(n, Act::Named(format!("{RUN_PREFIX}{n}")));
                match crate::keybindings::saved_macro_name(n) {
                    Some(name) => item.bound(Target::MacroFile(crate::keybindings::intern(name))),
                    None => item,
                }
            })
            .collect();
        if runs.is_empty() {
            runs.push(Item::new("No Saved Macros", Act::Named(String::new())).enabled(false));
        }
        runs
    }

    /// Tools ▸ Save Macro…: name the last recorded macro and save it.
    pub(super) fn save_macro(&mut self) {
        let Some(config) = self.config.clone() else { return };
        if self.macro_last.is_empty() {
            self.status = "No macro recorded yet: Tools ▸ Record Macro first".into();
            return;
        }
        let dir = macros_dir(&config);
        let _ = std::fs::create_dir_all(&dir);
        let picked = rfd::FileDialog::new().set_title("Save Macro").set_directory(&dir).set_file_name(format!("Macro.{EXTENSION}")).add_filter("Strata macro", &[EXTENSION]).save_file();
        let Some(mut path) = picked else { return };
        if path.extension().is_none_or(|e| e != EXTENSION) {
            path.set_extension(EXTENSION);
        }
        match write_macro(&path, &self.macro_last) {
            Ok(()) => {
                let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                self.status = if path.parent() == Some(dir.as_path()) {
                    format!("Saved the macro “{name}”: Tools ▸ Macros runs it")
                } else {
                    format!("Saved the macro to {} (only macros in {} appear in Tools ▸ Macros)", path.display(), dir.display())
                };
            }
            Err(e) => self.status = format!("Couldn't save the macro: {e}"),
        }
    }

    /// Play a saved macro at the cursor(s), like Playback Macro.
    pub(super) fn run_macro_file(&mut self, name: &str) {
        let Some(config) = self.config.clone() else { return };
        match read_macro(&config, name) {
            Ok(events) => {
                self.docs[self.active].view.focus();
                self.pending_events.extend(events);
            }
            Err(e) => self.status = e,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(key: Key, modifiers: Modifiers) -> Event {
        Event::Key { key, physical_key: Some(key), pressed: true, repeat: false, modifiers }
    }

    #[test]
    fn events_round_trip_through_json() {
        let events = vec![
            Event::Text("héllo".into()),
            key(Key::Enter, Modifiers::NONE),
            key(Key::ArrowLeft, Modifiers::SHIFT | Modifiers::ALT),
            key(Key::D, Modifiers::COMMAND | Modifiers::MAC_CMD),
            Event::Paste("a\nb".into()),
        ];
        let json = serde_json::to_string(&MacroFile::from_events(&events)).unwrap();
        assert!(json.contains(r#"{"key":"Enter"}"#) && json.contains(r#""mods":["alt","shift"]"#), "{json}");
        let back: MacroFile = serde_json::from_str(&json).unwrap();
        assert_eq!(back.events().unwrap(), events);
        // Releases and pointer input aren't recorded.
        let mut released = key(Key::A, Modifiers::NONE);
        if let Event::Key { pressed, .. } = &mut released {
            *pressed = false;
        }
        assert!(MacroFile::from_events(&[released, Event::PointerGone]).steps.is_empty());
        let bad: MacroFile = serde_json::from_str(r#"{"steps": [{"key": "Nope"}]}"#).unwrap();
        assert!(bad.events().unwrap_err().contains("Nope"));
    }

    #[test]
    fn names_from_keybindings() {
        assert_eq!(crate::keybindings::saved_macro_name("wrap"), Some("wrap"));
        assert_eq!(crate::keybindings::saved_macro_name("wrap.strata-macro"), Some("wrap"));
        assert_eq!(crate::keybindings::saved_macro_name("res://Packages/User/My Thing.sublime-macro"), Some("My Thing"));
        assert_eq!(crate::keybindings::saved_macro_name("..\\..\\x"), Some("x"), "only the last part counts: it stays in Macros");
        assert_eq!(crate::keybindings::saved_macro_name(".."), None);
        assert_eq!(crate::keybindings::saved_macro_name(""), None);
    }

    #[test]
    fn saved_macros_are_listed_and_play() {
        let config = std::env::temp_dir().join(format!("strata-macros-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&config);
        let ctx = egui::Context::default();
        let mut app = StrataApp::new(None, None);
        app.config = Some(config.clone());
        assert!(matches!(&app.saved_macro_items()[0], Item::Action { enabled: false, .. }), "nothing saved");
        app.macro_last = vec![Event::Text("x".into()), key(Key::Enter, Modifiers::NONE)];
        write_macro(&macros_dir(&config).join("Add X.strata-macro"), &app.macro_last).unwrap();
        std::fs::write(macros_dir(&config).join("notes.txt"), "").unwrap();
        assert_eq!(saved_macros(&config), ["Add X"]);
        let runs = app.saved_macro_items();
        assert!(matches!(&runs[0], Item::Action { label, act: Act::Named(n), .. } if label == "Add X" && n == "macro:Add X"));

        app.run_act(&ctx, Act::Named("macro:Add X".into()));
        assert_eq!(app.pending_events, app.macro_last);
        app.pending_events.clear();
        app.run_macro_file("Missing");
        assert!(app.status.starts_with("No saved macro named “Missing”"), "{}", app.status);
        assert!(app.pending_events.is_empty());

        // Bound in keybindings.json.
        let (keymap, errors) = crate::keybindings::Keymap::with_user(r#"[{"keys": ["ctrl+alt+x"], "command": "run_macro_file", "args": {"file": "Add X"}}]"#).unwrap();
        assert!(errors.is_empty(), "{errors:?}");
        let target = crate::keybindings::resolve("run_macro_file", serde_json::json!({"file": "Add X"}).as_object().unwrap());
        assert_eq!(target, Ok(crate::keybindings::What::Run(crate::keybindings::Target::MacroFile("Add X"))));
        assert!(keymap.label(crate::keybindings::Target::MacroFile("Add X")).is_some());
        app.run_target(&ctx, crate::keybindings::Target::MacroFile("Add X"));
        assert_eq!(app.pending_events, app.macro_last);
        // Sublime's own macros still map to editor commands.
        let sublime = crate::keybindings::resolve("run_macro_file", serde_json::json!({"file": "res://Packages/Default/Delete Line.sublime-macro"}).as_object().unwrap());
        assert_eq!(sublime, Ok(crate::keybindings::What::Run(crate::keybindings::Target::Editor("delete_line"))));
        std::fs::remove_dir_all(config).ok();
    }
}
