//! Where Strata keeps state between runs: user settings, plus the hot-exit
//! session (open tabs, and the text of unsaved ones) so quitting never asks.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Per-user config folder. `STRATA_CONFIG_DIR` overrides it (tests, portable installs).
pub fn config_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("STRATA_CONFIG_DIR") {
        return Some(dir.into());
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if cfg!(target_os = "macos") {
        home.map(|h| h.join("Library/Application Support/Strata"))
    } else if cfg!(windows) {
        std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("Strata"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or(home.map(|h| h.join(".config"))).map(|c| c.join("strata"))
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub font_size: f32,
    /// Editor font: a name from the font picker, or a font file path. Empty = built-in.
    pub font_face: String,
    /// Menus, tabs, sidebar and panels.
    pub ui_font_face: String,
    pub ui_font_size: f32,
    /// "line", "block" or "underline".
    pub caret_style: String,
    /// "blink", "smooth" (fades) or "solid".
    pub caret_blink: String,
    pub caret_width: f32,
    /// "selection" (dots and arrows inside selections), "all" or "none".
    pub draw_white_space: String,
    pub highlight_trailing_whitespace: bool,
    /// Color bracket pairs by nesting depth.
    pub bracket_colors: bool,
    /// Small color bars under CSS colors like #22d3ee.
    pub color_swatches: bool,
    /// Pin the enclosing block headers at the top while scrolling.
    pub sticky_scroll: bool,
    pub smooth_scroll: bool,
    /// Git change bars in the gutter.
    pub git_gutter: bool,
    /// 0.3 (see-through) to 1.0 (opaque).
    pub window_opacity: f32,
    pub theme: String,
    pub tab_size: usize,
    pub translate_tabs_to_spaces: bool,
    pub minimap: bool,
    /// Live Markdown preview beside the editor.
    pub markdown_preview: bool,
    pub show_tabs: bool,
    pub show_status_bar: bool,
    /// Soft-wrap every file. Prose syntaxes (Markdown, plain text…) wrap regardless.
    pub word_wrap: bool,
    /// Soft-wrap at this column instead of the window edge.
    pub wrap_column: Option<usize>,
    /// Vertical guides at these columns.
    pub rulers: Vec<usize>,
    /// Column for Wrap Paragraph at Ruler.
    pub wrap_width: usize,
    /// Save every modified file before building.
    pub save_all_on_build: bool,
    pub trim_trailing_white_space_on_save: bool,
    pub ensure_newline_at_eof_on_save: bool,
    /// Save modified files when the window loses focus.
    pub save_on_focus_lost: bool,
    /// Offer word completions while typing (Ctrl+Space always works).
    pub auto_complete: bool,
    /// Detect tabs vs spaces (and the width) from each file when it opens.
    pub detect_indentation: bool,
    /// Restore open tabs, unsaved text included, instead of asking on quit.
    pub hot_exit: bool,
    /// The terminal panel was open (a shell starts on launch only then).
    pub terminal_visible: bool,
    pub terminal_height: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            font_size: 14.0,
            font_face: String::new(),
            ui_font_face: String::new(),
            ui_font_size: 13.0,
            caret_style: "line".into(),
            caret_blink: "blink".into(),
            caret_width: 2.0,
            window_opacity: 1.0,
            draw_white_space: "selection".into(),
            highlight_trailing_whitespace: true,
            bracket_colors: true,
            color_swatches: true,
            sticky_scroll: true,
            smooth_scroll: true,
            git_gutter: true,
            theme: crate::highlight::DEFAULT_THEME.into(),
            tab_size: 4,
            translate_tabs_to_spaces: true,
            minimap: true,
            markdown_preview: false,
            show_tabs: true,
            show_status_bar: true,
            word_wrap: false,
            wrap_width: 80,
            wrap_column: None,
            rulers: Vec::new(),
            detect_indentation: true,
            auto_complete: true,
            save_all_on_build: true,
            trim_trailing_white_space_on_save: false,
            ensure_newline_at_eof_on_save: false,
            save_on_focus_lost: false,
            hot_exit: true,
            terminal_visible: false,
            terminal_height: 240.0,
        }
    }
}

impl Settings {
    pub fn path(dir: &Path) -> PathBuf {
        dir.join("settings.json")
    }

    /// Missing or broken files fall back to defaults; the error is returned for the status bar.
    pub fn load(dir: &Path) -> (Self, Option<String>) {
        match std::fs::read_to_string(Self::path(dir)) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(s) => (s, None),
                Err(e) => (Self::default(), Some(format!("settings.json: {e}"))),
            },
            Err(_) => (Self::default(), None),
        }
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        std::fs::write(Self::path(dir), serde_json::to_string_pretty(self)? + "\n")
    }
}

#[derive(Serialize, Deserialize, Default, Debug, PartialEq)]
#[serde(default)]
pub struct Session {
    pub folder: Option<PathBuf>,
    pub sidebar_visible: bool,
    pub active: usize,
    pub tabs: Vec<TabState>,
    pub layout: Option<crate::panes::Layout>,
    pub recent_files: Vec<PathBuf>,
    pub recent_folders: Vec<PathBuf>,
    pub recent_projects: Vec<PathBuf>,
}

#[derive(Serialize, Deserialize, Default, Debug, PartialEq)]
#[serde(default)]
pub struct TabState {
    pub path: Option<PathBuf>,
    /// File in `backups/` holding unsaved text, if the tab was dirty.
    pub backup: Option<String>,
    pub cursor: usize,
    pub syntax: Option<String>,
    /// The file's modification time when the tab last matched it, to spot
    /// changes made while Strata was closed.
    pub disk_mtime: Option<std::time::SystemTime>,
    /// The file changed on disk while the tab had unsaved edits.
    pub conflict: bool,
    pub group: usize,
    pub wrap: Option<bool>,
    /// Encoding id (see `Encoding::id`) to save an unsaved tab with.
    pub encoding: Option<String>,
    /// Tabs shown in each pane: the highest stamp per group.
    pub stamp: u64,
}

impl Session {
    fn path(dir: &Path) -> PathBuf {
        dir.join("session.json")
    }

    pub fn backups(dir: &Path) -> PathBuf {
        dir.join("backups")
    }

    /// `Ok(None)` when there is no session yet; `Err` when one exists but can't be read.
    pub fn load(dir: &Path) -> Result<Option<Self>, String> {
        match std::fs::read_to_string(Self::path(dir)) {
            Ok(text) => serde_json::from_str(&text).map(Some).map_err(|e| format!("session.json: {e}")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("session.json: {e}")),
        }
    }

    /// Move every backup aside so no later save prunes text that couldn't be
    /// restored. Returns the folder they went to.
    pub fn quarantine_backups(dir: &Path) -> std::io::Result<PathBuf> {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let to = dir.join(format!("backups.recovered-{stamp}"));
        std::fs::rename(Self::backups(dir), &to)?;
        Ok(to)
    }

    /// A backup name never used before, so an old session.json can't point
    /// at a file that now holds another tab's text.
    pub fn backup_name(tab: usize) -> String {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        format!("{nanos}-{tab}.txt")
    }

    /// Every file is flushed to disk before session.json is replaced, and
    /// stale backups are pruned only after that, so a crash at any point
    /// leaves either the old or the new session intact.
    pub fn save(&self, dir: &Path, backups: &[(String, String)]) -> std::io::Result<()> {
        let bdir = Self::backups(dir);
        std::fs::create_dir_all(&bdir)?;
        for (name, text) in backups {
            write_synced(&bdir.join(name), text)?;
        }
        let tmp = dir.join("session.json.tmp");
        write_synced(&tmp, &serde_json::to_string_pretty(self)?)?;
        std::fs::rename(&tmp, Self::path(dir))?;
        #[cfg(unix)]
        std::fs::File::open(dir)?.sync_all()?;
        // Drop backups no tab refers to any more.
        let keep: Vec<&str> = self.tabs.iter().filter_map(|t| t.backup.as_deref()).collect();
        for entry in std::fs::read_dir(&bdir)?.flatten() {
            if !keep.contains(&entry.file_name().to_string_lossy().as_ref()) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
        Ok(())
    }
}

fn write_synced(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::File::create(path)?;
    f.write_all(text.as_bytes())?;
    f.sync_all()
}

/// Held for the app's lifetime: only one running Strata owns the session.
pub struct SessionLock(#[allow(dead_code)] std::fs::File);

impl SessionLock {
    /// `None` if another Strata already holds it.
    pub fn acquire(dir: &Path) -> Option<Self> {
        std::fs::create_dir_all(dir).ok()?;
        let f = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(dir.join("session.lock")).ok()?;
        f.try_lock().ok()?;
        Some(Self(f))
    }
}

impl Drop for SessionLock {
    /// Unlock explicitly: a child process forked meanwhile (builds) shares the
    /// open file, and closing our handle alone would leave the lock held.
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_round_trip_prunes_stale_backups() {
        let dir = std::env::temp_dir().join(format!("strata-session-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let s = Session {
            active: 1,
            tabs: vec![
                TabState { path: Some("/x/a.rs".into()), cursor: 5, ..Default::default() },
                TabState { backup: Some("1.txt".into()), ..Default::default() },
            ],
            ..Default::default()
        };
        std::fs::create_dir_all(Session::backups(&dir)).unwrap();
        std::fs::write(Session::backups(&dir).join("old.txt"), "stale").unwrap();
        s.save(&dir, &[("1.txt".into(), "unsaved".into())]).unwrap();
        let back = Session::load(&dir).unwrap().unwrap();
        assert_eq!(back.active, 1);
        assert_eq!(back.tabs[0].cursor, 5);
        assert_eq!(std::fs::read_to_string(Session::backups(&dir).join("1.txt")).unwrap(), "unsaved");
        assert!(!Session::backups(&dir).join("old.txt").exists());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn broken_session_is_an_error_and_backups_can_be_set_aside() {
        let dir = std::env::temp_dir().join(format!("strata-session-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(Session::load(&dir), Ok(None));
        std::fs::create_dir_all(Session::backups(&dir)).unwrap();
        std::fs::write(Session::backups(&dir).join("1-0.txt"), "precious").unwrap();
        std::fs::write(dir.join("session.json"), "").unwrap();
        assert!(Session::load(&dir).is_err());
        let moved = Session::quarantine_backups(&dir).unwrap();
        assert_eq!(std::fs::read_to_string(moved.join("1-0.txt")).unwrap(), "precious");
        assert_ne!(Session::backup_name(0), Session::backup_name(0));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn only_one_instance_holds_the_lock() {
        let dir = std::env::temp_dir().join(format!("strata-lock-{}", std::process::id()));
        let a = SessionLock::acquire(&dir);
        assert!(a.is_some());
        assert!(SessionLock::acquire(&dir).is_none());
        drop(a);
        assert!(SessionLock::acquire(&dir).is_some());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn settings_tolerate_partial_and_broken_files() {
        let dir = std::env::temp_dir().join(format!("strata-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(Settings::path(&dir), r#"{ "font_size": 18 }"#).unwrap();
        let (s, err) = Settings::load(&dir);
        assert_eq!((s.font_size, s.tab_size, err), (18.0, 4, None));
        std::fs::write(Settings::path(&dir), "{ nope").unwrap();
        let (s, err) = Settings::load(&dir);
        assert_eq!(s, Settings::default());
        assert!(err.is_some());
        std::fs::remove_dir_all(dir).ok();
    }
}
