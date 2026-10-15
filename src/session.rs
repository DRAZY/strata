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
    /// Help ▸ Documentation opens in its own window (true) or inside the editor window.
    pub help_in_own_window: bool,
    pub theme: String,
    /// "fixed" uses `theme`; "auto" follows the system's light or dark
    /// appearance with `light_theme` / `dark_theme`; "dark" / "light" pin one.
    pub theme_mode: String,
    pub dark_theme: String,
    pub light_theme: String,
    /// Colors over every scheme, e.g. {"background": "#1e1e2e", "comment": "#7f849c"}.
    pub theme_overrides: std::collections::BTreeMap<String, String>,
    /// The same, for one scheme: {"Nord": {"selection": "#434c5e"}}.
    pub theme_overrides_by_theme: std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>,
    /// Line height as a multiple of the font's own (1.0 = unchanged).
    pub line_height: f32,
    /// Extra space above and below each line, in points (Sublime's names).
    pub line_padding_top: f32,
    pub line_padding_bottom: f32,
    /// "no_italic", "no_underline" ("no_bold" is accepted; code is never bold).
    pub font_options: Vec<String>,
    pub tab_size: usize,
    pub translate_tabs_to_spaces: bool,
    /// Enter keeps the current line's indentation.
    pub auto_indent: bool,
    /// Per-language rules: indent after `{`/`:`/open tags, dedent closers.
    pub smart_indent: bool,
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
    /// Save edited files after this many milliseconds without typing (0: off).
    pub auto_save_delay_ms: u64,
    /// Offer word completions while typing (Ctrl+Space always works).
    pub auto_complete: bool,
    /// Detect tabs vs spaces (and the width) from each file when it opens.
    pub detect_indentation: bool,
    /// Restore open tabs, unsaved text included, instead of asking on quit.
    pub hot_exit: bool,
    /// Files bigger than this (MB) open in large-file mode: plain text, no
    /// minimap, word wrap or bracket colors.
    pub large_file_mb: f64,
    /// Ask before opening a file bigger than this (MB).
    pub large_file_warning_mb: f64,
    /// The terminal panel was open (a shell starts on launch only then).
    pub terminal_visible: bool,
    pub terminal_height: f32,
    /// Let programs in the terminal read the clipboard (OSC 52).
    pub terminal_allow_clipboard_read: bool,
    /// Let programs in the terminal set the clipboard (OSC 52); each time, the status bar says so.
    pub terminal_allow_clipboard_write: bool,
    /// Alt/Option sends Esc + key (Meta) instead of typing the character the
    /// keyboard layout gives. Off on macOS (Option types @, [, ], é… on many
    /// layouts), on elsewhere.
    pub terminal_option_as_meta: bool,
    /// Opening the find bar with a multi-line selection turns on In Selection.
    pub auto_find_in_selection: bool,
    /// Underline misspelled words in prose: comments, strings, Markdown, plain text.
    pub spell_check: bool,
    /// "en_US" or "en_GB".
    pub dictionary: String,
    /// Words spell check accepts (Add Word to Dictionary adds here).
    pub added_words: Vec<String>,
    /// Scopes spell check looks at (a Sublime scope selector).
    pub spelling_selector: String,
    /// The side bar hides what .gitignore ignores.
    pub sidebar_respect_gitignore: bool,
    /// Names (globs) the side bar never shows.
    pub file_exclude_patterns: Vec<String>,
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
            help_in_own_window: true,
            draw_white_space: "selection".into(),
            highlight_trailing_whitespace: true,
            bracket_colors: true,
            color_swatches: true,
            sticky_scroll: true,
            smooth_scroll: true,
            git_gutter: true,
            theme: crate::highlight::DEFAULT_THEME.into(),
            theme_mode: "fixed".into(),
            dark_theme: crate::highlight::DEFAULT_THEME.into(),
            light_theme: "Strata Light".into(),
            theme_overrides: Default::default(),
            theme_overrides_by_theme: Default::default(),
            line_height: 1.0,
            line_padding_top: 0.0,
            line_padding_bottom: 0.0,
            font_options: Vec::new(),
            tab_size: 4,
            translate_tabs_to_spaces: true,
            auto_indent: true,
            smart_indent: true,
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
            auto_save_delay_ms: 0,
            hot_exit: true,
            large_file_mb: (crate::document::LARGE_FILE_BYTES >> 20) as f64,
            large_file_warning_mb: 500.0,
            terminal_visible: false,
            terminal_height: 240.0,
            terminal_allow_clipboard_read: false,
            terminal_allow_clipboard_write: true,
            terminal_option_as_meta: !cfg!(target_os = "macos"),
            auto_find_in_selection: false,
            spell_check: false,
            dictionary: crate::spell::DEFAULT_DICTIONARY.into(),
            added_words: Vec::new(),
            spelling_selector: crate::spell::DEFAULT_SELECTOR.into(),
            sidebar_respect_gitignore: true,
            file_exclude_patterns: vec![".git".into(), ".DS_Store".into()],
        }
    }
}

/// JSON with `//` and `/* */` comments and trailing commas, as Sublime's settings
/// files allow, turned into strict JSON. Strings are left alone.
pub fn strip_jsonc(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    let mut start = 0;
    while i < b.len() {
        match b[i] {
            b'"' => {
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                i += 1;
            }
            b'/' if b.get(i + 1) == Some(&b'/') => {
                out.push_str(&s[start..i]);
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
                start = i;
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                out.push_str(&s[start..i]);
                i = s[i + 2..].find("*/").map_or(b.len(), |e| i + 2 + e + 2);
                out.push(' ');
                start = i;
            }
            b',' => {
                let rest = s[i + 1..].trim_start();
                // A comma before a closing bracket (allowing comments in between) is dropped.
                let next = strip_jsonc_peek(rest);
                if matches!(next, Some(b'}') | Some(b']')) {
                    out.push_str(&s[start..i]);
                    start = i + 1;
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    out.push_str(&s[start.min(s.len())..]);
    out
}

/// First significant byte, skipping whitespace and comments.
fn strip_jsonc_peek(mut s: &str) -> Option<u8> {
    loop {
        s = s.trim_start();
        if let Some(rest) = s.strip_prefix("//") {
            s = rest.find('\n').map_or("", |n| &rest[n..]);
        } else if let Some(rest) = s.strip_prefix("/*") {
            s = rest.find("*/").map_or("", |n| &rest[n + 2..]);
        } else {
            return s.bytes().next();
        }
    }
}

impl Settings {
    pub fn path(dir: &Path) -> PathBuf {
        dir.join("settings.json")
    }

    /// Missing or broken files fall back to defaults; the error is returned for the status bar.
    /// Like Sublime, comments and trailing commas are allowed.
    pub fn load(dir: &Path) -> (Self, Option<String>) {
        match std::fs::read_to_string(Self::path(dir)) {
            Ok(text) => match Self::parse(&text) {
                Ok(s) => (s, None),
                Err(e) => (Self::default(), Some(e)),
            },
            Err(_) => (Self::default(), None),
        }
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        serde_json::from_str(&strip_jsonc(text)).map_err(|e| format!("settings.json: {e}"))
    }

    /// Writes the settings. An existing file is edited in place: only the
    /// values that changed are rewritten (a setting that isn't in the file is
    /// added only if it differs from the default), so comments, order and
    /// keys Strata doesn't know all stay. Refuses to touch a file that doesn't
    /// parse, so a typo never costs the user their settings. A new file gets
    /// every setting.
    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        crate::private::create_dir_all(dir)?;
        let path = Self::path(dir);
        let Some(text) = self.settings_text(&path)? else { return Ok(()) };
        crate::buffer::Buffer::from_str(&text).write_to(&path, crate::encoding::Encoding::UTF8)?;
        crate::private::restrict(&path, false);
        Ok(())
    }

    /// The new settings.json text, or None when the file already says it all.
    fn settings_text(&self, path: &Path) -> std::io::Result<Option<String>> {
        use serde_json::Value;
        let broken = || std::io::Error::other("settings.json has errors; fix them first so your settings aren't overwritten");
        let Value::Object(ours) = serde_json::to_value(self)? else { return Err(broken()) };
        let original = match std::fs::read_to_string(path) {
            Ok(text) if !text.trim().is_empty() => text,
            _ => return Ok(Some(serde_json::to_string_pretty(&ours)? + "\n")),
        };
        let Ok(Value::Object(file)) = serde_json::from_str::<Value>(&strip_jsonc(&original)) else { return Err(broken()) };
        let Value::Object(defaults) = serde_json::to_value(Settings::default())? else { return Err(broken()) };
        let mut text = original.clone();
        for (key, value) in &ours {
            let changed = match file.get(key) {
                Some(old) => !crate::jsonc::same(old, value),
                None => !defaults.get(key).is_some_and(|d| crate::jsonc::same(d, value)),
            };
            if changed {
                text = crate::jsonc::set_key(&text, key, value).ok_or_else(broken)?;
            }
        }
        Ok((text != original).then_some(text))
    }
}

#[derive(Serialize, Deserialize, Default, Debug, PartialEq)]
#[serde(default)]
pub struct Session {
    /// The first window, which was the only one before Strata had several
    /// (kept so an older Strata still restores it).
    pub folder: Option<PathBuf>,
    pub sidebar_visible: bool,
    pub active: usize,
    pub tabs: Vec<TabState>,
    pub layout: Option<crate::panes::Layout>,
    pub recent_files: Vec<PathBuf>,
    pub recent_folders: Vec<PathBuf>,
    pub recent_projects: Vec<PathBuf>,
    /// Find bar and Find in Files history (see `find_history`).
    #[serde(skip_serializing_if = "crate::find_history::FindHistory::is_empty")]
    pub find_history: crate::find_history::FindHistory,
    /// Every window, first one first. Empty in sessions saved before Strata
    /// had several windows: the fields above are then the one window.
    pub windows: Vec<WindowState>,
}

/// One window: its tabs, panes and folder, and where it was on screen.
#[derive(Serialize, Deserialize, Default, Debug, PartialEq, Clone)]
#[serde(default)]
pub struct WindowState {
    /// The first folder, which was the only one before Strata had several
    /// (kept so an older Strata still opens it).
    pub folder: Option<PathBuf>,
    /// Every folder in the sidebar, first one first.
    pub folders: Vec<PathBuf>,
    pub sidebar_visible: bool,
    pub active: usize,
    pub tabs: Vec<TabState>,
    pub layout: Option<crate::panes::Layout>,
    /// Top-left corner of the window frame, in points.
    pub position: Option<[f32; 2]>,
    /// Size of the content area, in points.
    pub size: Option<[f32; 2]>,
    /// The find bar's options and last search.
    pub find: crate::find::FindOptions,
}

#[derive(Serialize, Deserialize, Default, Debug, PartialEq, Clone)]
#[serde(default)]
pub struct TabState {
    pub path: Option<PathBuf>,
    /// File in `backups/` holding unsaved text, if the tab was dirty.
    pub backup: Option<String>,
    /// The primary cursor (all a session written before `selections` has).
    pub cursor: usize,
    /// Every selection as (anchor, head) in chars; empty in older sessions.
    pub selections: Vec<(usize, usize)>,
    /// Which of `selections` is the primary one.
    pub primary: usize,
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
    /// The unsaved text holds U+FFFD for bytes that didn't decode (Save warns).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub lossy: bool,
    /// "lf" or "crlf": the ending new lines get (None: detect from the text).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_ending: Option<String>,
    /// Tabs shown in each pane: the highest stamp per group.
    pub stamp: u64,
    /// Another view of the buffer of tab `n` (earlier in `tabs`), which holds
    /// the path and any backup (File ▸ New View into File).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clone_of: Option<usize>,
}

impl Session {
    fn path(dir: &Path) -> PathBuf {
        dir.join("session.json")
    }

    pub fn backups(dir: &Path) -> PathBuf {
        dir.join("backups")
    }

    /// A session of these windows (and the find history every window
    /// shares); the first window is also written the old way.
    pub fn of_windows(windows: Vec<WindowState>) -> Self {
        let first = windows.first().cloned().unwrap_or_default();
        Self { folder: first.folder, sidebar_visible: first.sidebar_visible, active: first.active, tabs: first.tabs, layout: first.layout, windows, find_history: crate::find_history::snapshot(), ..Default::default() }
    }

    /// The windows to reopen. An old single-window session is one window.
    pub fn window_states(&self) -> Vec<WindowState> {
        if !self.windows.is_empty() {
            return self.windows.clone();
        }
        vec![WindowState { folder: self.folder.clone(), folders: Vec::new(), sidebar_visible: self.sidebar_visible, active: self.active, tabs: self.tabs.clone(), layout: self.layout.clone(), position: None, size: None, find: Default::default() }]
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
        // The clock may not tick between two calls: a counter keeps names apart.
        static CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        format!("{nanos}-{n}-{tab}.txt")
    }

    /// The backup name for one exact text (`Editor::text_id`, unique in
    /// this process; the prefix sets this run apart from earlier ones). A
    /// backup already written under it holds that text, so it isn't
    /// written again until the tab changes.
    pub fn backup_name_for(text_id: u64) -> String {
        static RUN: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| Session::backup_name(std::process::id() as usize).replace(".txt", ""));
        format!("{}-t{text_id}.txt", *RUN)
    }

    /// Every file is flushed to disk before session.json is replaced, and
    /// stale backups are pruned only after that, so a crash at any point
    /// leaves either the old or the new session intact. Backups are rope
    /// snapshots (taking one costs nothing on the UI thread), and one whose
    /// file exists already is skipped: names are never reused for other text.
    pub fn save(&self, dir: &Path, backups: &[(String, ropey::Rope)]) -> std::io::Result<()> {
        self.save_with(dir, backups, true)
    }

    /// `save`, but keeping backups no tab refers to any more: after a crash
    /// the state being saved may be damaged, so the last good backups stay
    /// (the next ordinary save prunes them).
    pub fn save_keeping_backups(&self, dir: &Path, backups: &[(String, ropey::Rope)]) -> std::io::Result<()> {
        self.save_with(dir, backups, false)
    }

    fn save_with(&self, dir: &Path, backups: &[(String, ropey::Rope)], prune: bool) -> std::io::Result<()> {
        let bdir = Self::backups(dir);
        crate::private::create_dir_all(&bdir)?;
        for (name, text) in backups {
            let path = bdir.join(name);
            if !path.exists() {
                write_backup(&path, text)?;
            }
        }
        let tmp = dir.join("session.json.tmp");
        write_synced(&tmp, &serde_json::to_string_pretty(self)?)?;
        std::fs::rename(&tmp, Self::path(dir))?;
        #[cfg(unix)]
        std::fs::File::open(dir)?.sync_all()?;
        if !prune {
            return Ok(());
        }
        // Drop backups no tab refers to any more.
        let keep: Vec<&str> = self.tabs.iter().chain(self.windows.iter().flat_map(|w| &w.tabs)).filter_map(|t| t.backup.as_deref()).collect();
        for entry in std::fs::read_dir(&bdir)?.flatten() {
            if !keep.contains(&entry.file_name().to_string_lossy().as_ref()) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
        Ok(())
    }
}

/// Written under a temporary name and renamed, so a backup file that exists is complete.
fn write_backup(path: &Path, text: &ropey::Rope) -> std::io::Result<()> {
    use std::io::Write;
    let tmp = path.with_extension("partial");
    let mut f = std::io::BufWriter::new(crate::private::create(&tmp)?);
    for chunk in text.chunks() {
        f.write_all(chunk.as_bytes())?;
    }
    f.into_inner().map_err(|e| e.into_error())?.sync_all()?;
    std::fs::rename(&tmp, path)
}

fn write_synced(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = crate::private::create(path)?;
    f.write_all(text.as_bytes())?;
    f.sync_all()
}

/// Held for the app's lifetime: only one running Strata owns the session.
pub struct SessionLock(#[allow(dead_code)] std::fs::File);

impl SessionLock {
    /// `None` if another Strata already holds it.
    pub fn acquire(dir: &Path) -> Option<Self> {
        crate::private::create_dir_all(dir).ok()?;
        let f = crate::private::options().create(true).truncate(false).write(true).open(dir.join("session.lock")).ok()?;
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
    fn several_windows_round_trip_and_keep_every_windows_backups() {
        let dir = std::env::temp_dir().join(format!("strata-session-windows-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let one = WindowState { tabs: vec![TabState { path: Some("/x/a.rs".into()), ..Default::default() }], position: Some([10.0, 20.0]), ..Default::default() };
        let two = WindowState { active: 1, tabs: vec![TabState::default(), TabState { backup: Some("2.txt".into()), ..Default::default() }], size: Some([800.0, 600.0]), ..Default::default() };
        let s = Session::of_windows(vec![one.clone(), two.clone()]);
        // The first window is also where an older Strata looks for it.
        assert_eq!(s.tabs, one.tabs);
        s.save(&dir, &[("2.txt".into(), "second window".into())]).unwrap();
        let back = Session::load(&dir).unwrap().unwrap();
        assert_eq!(back.window_states(), vec![one, two]);
        assert_eq!(std::fs::read_to_string(Session::backups(&dir).join("2.txt")).unwrap(), "second window", "not pruned");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_session_from_before_windows_is_one_window() {
        let old = r#"{"folder": "/p", "sidebar_visible": true, "active": 1, "tabs": [{"path": "/p/a"}, {"backup": "1-0.txt"}], "recent_files": ["/p/a"]}"#;
        let s: Session = serde_json::from_str(old).unwrap();
        let w = s.window_states();
        assert_eq!(w.len(), 1);
        assert_eq!((w[0].folder.as_deref(), w[0].sidebar_visible, w[0].active, w[0].tabs.len()), (Some(Path::new("/p")), true, 1, 2));
        assert_eq!((w[0].position, w[0].size), (None, None));
        assert_eq!(s.recent_files, vec![PathBuf::from("/p/a")]);
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
        // A broken file is never overwritten.
        assert!(s.save(&dir).is_err());
        assert_eq!(std::fs::read_to_string(Settings::path(&dir)).unwrap(), "{ nope");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn saving_settings_edits_the_file_in_place() {
        let dir = std::env::temp_dir().join(format!("strata-settings-jsonc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let text = "// Strata settings\n{\n    // bigger\n    \"font_size\": 18,\n    \"my_plugin_key\": true, /* unknown to Strata */\n    \"rulers\": [80],\n}\n";
        std::fs::write(Settings::path(&dir), text).unwrap();
        let (mut s, err) = Settings::load(&dir);
        assert_eq!(err, None);
        // Nothing changed: the file isn't touched.
        s.save(&dir).unwrap();
        assert_eq!(std::fs::read_to_string(Settings::path(&dir)).unwrap(), text);
        // Zoom: only that value changes.
        s.font_size = 19.0;
        s.save(&dir).unwrap();
        assert_eq!(std::fs::read_to_string(Settings::path(&dir)).unwrap(), text.replace("18,", "19.0,"));
        // A View toggle that isn't in the file yet: added, nothing else is.
        s.minimap = false;
        s.save(&dir).unwrap();
        let after = std::fs::read_to_string(Settings::path(&dir)).unwrap();
        assert!(after.starts_with("// Strata settings\n{\n    // bigger\n") && after.contains("/* unknown to Strata */"), "{after}");
        assert!(after.contains("    \"minimap\": false,\n}"), "{after}");
        assert!(!after.contains("tab_size"), "defaults aren't written out");
        let (back, err) = Settings::load(&dir);
        assert_eq!((back.font_size, back.minimap, back.rulers.clone(), err), (19.0, false, vec![80], None));
        // A missing file gets every setting.
        std::fs::remove_file(Settings::path(&dir)).unwrap();
        s.save(&dir).unwrap();
        assert!(std::fs::read_to_string(Settings::path(&dir)).unwrap().contains("\"tab_size\": 4"));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn appearance_settings_parse_with_defaults() {
        let s = Settings::parse("{}").unwrap();
        assert_eq!((s.theme_mode.as_str(), s.line_height, s.line_padding_top), ("fixed", 1.0, 0.0));
        assert!(s.theme_overrides.is_empty() && s.font_options.is_empty());
        let text = r##"{
            "theme_mode": "auto", "dark_theme": "Tokyo Night", "light_theme": "Catppuccin Latte",
            // Colors over every scheme
            "theme_overrides": { "background": "#1e1e2e", "comment": "#7f849c", },
            "theme_overrides_by_theme": { "Nord": { "selection": "#434c5e" } },
            "line_height": 1.4, "line_padding_top": 2, "line_padding_bottom": 1,
            "font_options": ["no_italic"],
        }"##;
        let s = Settings::parse(text).unwrap();
        assert_eq!((s.theme_mode.as_str(), s.dark_theme.as_str(), s.light_theme.as_str()), ("auto", "Tokyo Night", "Catppuccin Latte"));
        assert_eq!(s.theme_overrides["comment"], "#7f849c");
        assert_eq!(s.theme_overrides_by_theme["Nord"]["selection"], "#434c5e");
        assert_eq!((s.line_height, s.line_padding_top, s.line_padding_bottom), (1.4, 2.0, 1.0));
        assert_eq!(s.font_options, ["no_italic"]);
    }

    #[test]
    fn settings_allow_comments_trailing_commas_and_keep_unknown_keys() {
        let text = "{\n  // size\n  \"font_size\": 16, /* \"tab_size\": 9, */\n  \"theme\": \"a // not a comment\",\n  \"rulers\": [80, 100,],\n  \"mine\": true,\n}\n";
        let s = Settings::parse(text).unwrap();
        assert_eq!((s.font_size, s.tab_size, s.theme.as_str(), s.rulers.clone()), (16.0, 4, "a // not a comment", vec![80, 100]));
        let dir = std::env::temp_dir().join(format!("strata-settings-keep-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(Settings::path(&dir), text).unwrap();
        s.save(&dir).unwrap();
        let text = std::fs::read_to_string(Settings::path(&dir)).unwrap();
        assert!(text.contains("// size") && text.contains("/* \"tab_size\": 9, */"), "comments are kept: {text}");
        let saved: serde_json::Value = serde_json::from_str(&strip_jsonc(&text)).unwrap();
        assert_eq!(saved["mine"], serde_json::Value::Bool(true));
        assert_eq!(saved["font_size"], serde_json::json!(16));
        std::fs::remove_dir_all(dir).ok();
    }
}
