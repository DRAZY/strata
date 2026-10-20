//! The Obsidian settings that change how links and new notes are made, read
//! (never written) from the vault's `.obsidian` folder: `app.json`,
//! `daily-notes.json` and `templates.json`. Missing files or keys mean
//! Obsidian's defaults; a file that isn't valid JSON is ignored the same way.

use super::resolve::LinkFormat;
use serde_json::Value;
use std::path::Path;

/// Where new notes go (Obsidian's "Default location for new notes").
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum NewNotes {
    /// The vault folder.
    #[default]
    Root,
    /// The folder of the note you're in.
    Current,
    /// This folder (relative to the vault).
    Folder(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Daily {
    pub folder: String,
    /// moment.js format; `/` in it makes folders.
    pub format: String,
    /// The template note, relative to the vault, `.md` optional.
    pub template: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Templates {
    /// None: no templates folder is set (Strata then looks for `Templates`).
    pub folder: Option<String>,
    pub date_format: String,
    pub time_format: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// The folder has `.obsidian`: it's a vault Obsidian opens.
    pub obsidian: bool,
    pub new_notes: NewNotes,
    pub markdown_links: bool,
    pub link_format: LinkFormat,
    pub daily: Daily,
    pub templates: Templates,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            obsidian: false,
            new_notes: NewNotes::Root,
            markdown_links: false,
            link_format: LinkFormat::Shortest,
            // Without daily-notes.json: Journal/2026-10-19.md.
            daily: Daily { folder: "Journal".into(), format: DEFAULT_DATE.into(), template: String::new() },
            templates: Templates { folder: None, date_format: DEFAULT_DATE.into(), time_format: "HH:mm".into() },
        }
    }
}

pub const DEFAULT_DATE: &str = "YYYY-MM-DD";

fn read_json(path: &Path) -> Option<serde_json::Map<String, Value>> {
    let text = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str::<Value>(&crate::session::strip_jsonc(&text)) {
        Ok(Value::Object(m)) => Some(m),
        _ => {
            log::warn!("ignoring {} (not a JSON object)", path.display());
            None
        }
    }
}

fn string(m: &serde_json::Map<String, Value>, key: &str) -> Option<String> {
    m.get(key).and_then(Value::as_str).map(str::to_owned)
}

/// A folder setting as a path relative to the vault (Obsidian writes them
/// with or without a leading or trailing `/`).
fn folder(s: &str) -> String {
    // `.` and `..` are dropped: a folder setting never leads out of the vault.
    s.trim().split(['/', '\\']).filter(|p| !p.is_empty() && *p != "." && *p != "..").collect::<Vec<_>>().join("/")
}

impl Config {
    pub fn read(root: &Path) -> Config {
        let mut c = Config::default();
        let dir = root.join(".obsidian");
        c.obsidian = dir.is_dir();
        if !c.obsidian {
            return c;
        }
        if let Some(app) = read_json(&dir.join("app.json")) {
            c.new_notes = match string(&app, "newFileLocation").as_deref() {
                Some("current") => NewNotes::Current,
                Some("folder") => {
                    let f = folder(&string(&app, "newFileFolderPath").unwrap_or_default());
                    if f.is_empty() { NewNotes::Root } else { NewNotes::Folder(f) }
                }
                _ => NewNotes::Root,
            };
            c.markdown_links = app.get("useMarkdownLinks").and_then(Value::as_bool).unwrap_or(false);
            c.link_format = match string(&app, "newLinkFormat").as_deref() {
                Some("relative") => LinkFormat::Relative,
                Some("absolute") => LinkFormat::Absolute,
                _ => LinkFormat::Shortest,
            };
        }
        // The Daily notes plugin's own defaults (vault folder) apply once it has settings.
        if let Some(d) = read_json(&dir.join("daily-notes.json")) {
            c.daily = Daily {
                folder: folder(&string(&d, "folder").unwrap_or_default()),
                format: string(&d, "format").filter(|f| !f.trim().is_empty()).unwrap_or_else(|| DEFAULT_DATE.into()),
                template: string(&d, "template").unwrap_or_default().trim().to_owned(),
            };
        }
        if let Some(t) = read_json(&dir.join("templates.json")) {
            c.templates.folder = string(&t, "folder").map(|f| folder(&f)).filter(|f| !f.is_empty());
            if let Some(f) = string(&t, "dateFormat").filter(|f| !f.trim().is_empty()) {
                c.templates.date_format = f;
            }
            if let Some(f) = string(&t, "timeFormat").filter(|f| !f.trim().is_empty()) {
                c.templates.time_format = f;
            }
        }
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_obsidian_settings_and_falls_back_to_defaults() {
        let dir = std::env::temp_dir().join(format!("strata-notes-config-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(Config::read(&dir), Config::default(), "a plain folder");
        let ob = dir.join(".obsidian");
        std::fs::create_dir_all(&ob).unwrap();
        std::fs::write(ob.join("app.json"), r#"{"newFileLocation":"folder","newFileFolderPath":"/Inbox/","useMarkdownLinks":true,"newLinkFormat":"relative","attachmentFolderPath":"./att"}"#).unwrap();
        std::fs::write(ob.join("daily-notes.json"), r#"{"folder":"Daily/","format":"YYYY/MM/YYYY-MM-DD ddd","template":"Templates/Day"}"#).unwrap();
        std::fs::write(ob.join("templates.json"), r#"{"folder":"Tpl","dateFormat":"DD.MM.YYYY"}"#).unwrap();
        let c = Config::read(&dir);
        assert!(c.obsidian && c.markdown_links);
        assert_eq!(c.new_notes, NewNotes::Folder("Inbox".into()));
        assert_eq!(c.link_format, LinkFormat::Relative);
        assert_eq!(c.daily, Daily { folder: "Daily".into(), format: "YYYY/MM/YYYY-MM-DD ddd".into(), template: "Templates/Day".into() });
        assert_eq!(c.templates, Templates { folder: Some("Tpl".into()), date_format: "DD.MM.YYYY".into(), time_format: "HH:mm".into() });
        // Broken or empty files: Obsidian's defaults (an empty daily-notes.json means the vault folder).
        std::fs::write(ob.join("app.json"), "{ not json").unwrap();
        std::fs::write(ob.join("daily-notes.json"), "{}").unwrap();
        let c = Config::read(&dir);
        assert_eq!((c.new_notes, c.markdown_links), (NewNotes::Root, false));
        assert_eq!(c.daily.folder, "");
        assert_eq!(c.daily.format, DEFAULT_DATE);
        assert_eq!(folder("../../outside/./x/"), "outside/x", "never out of the vault");
        std::fs::remove_dir_all(dir).ok();
    }
}
