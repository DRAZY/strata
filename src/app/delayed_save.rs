//! Auto save after a delay (`auto_save_delay_ms`, off at 0): once typing
//! has paused that long, the files edited since are saved, the way Save
//! would (trimming and the final newline included). Only what Save would
//! write without asking: never an untitled tab, a read-only one, a file
//! changed or deleted on disk, one with bytes that didn't decode, or text
//! the file's encoding can't hold. Those wait for a real Save.

use super::StrataApp;
use egui::Id;
use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub(super) struct DelayedSave {
    /// Each tab's editor version when last looked at (by view id). None
    /// until the first look, so text restored at startup isn't saved unasked.
    versions: Option<HashMap<Id, u64>>,
    /// Tabs edited since the last auto save.
    edited: HashSet<Id>,
    /// When the last edit was seen (input clock, seconds).
    last_edit: f64,
    /// An auto save is running and has done its own trimming: `before_save` skips it.
    pub(super) trimmed: bool,
}

impl StrataApp {
    /// Each frame, with the input clock: notice edits, and save once
    /// they've paused for the delay. Asks `ctx` to wake the window then.
    pub(super) fn delayed_save_tick(&mut self, ctx: Option<&egui::Context>, now: f64) {
        let delay = self.settings.auto_save_delay_ms;
        let versions: HashMap<Id, u64> = self.docs.iter().map(|d| (d.view.id, d.editor.version())).collect();
        let state = &mut self.delayed_save;
        if let Some(old) = &state.versions {
            for (id, v) in &versions {
                if old.get(id).is_some_and(|o| o != v) || (!old.contains_key(id) && *v > 0) {
                    state.edited.insert(*id);
                    state.last_edit = now;
                }
            }
        }
        state.versions = Some(versions);
        state.edited.retain(|id| self.docs.iter().any(|d| d.view.id == *id));
        if delay == 0 || state.edited.is_empty() {
            state.edited.clear();
            return;
        }
        let wait = delay as f64 / 1000.0 - (now - state.last_edit);
        if wait > 0.0 {
            if let Some(ctx) = ctx {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait));
            }
            return;
        }
        let edited = std::mem::take(&mut state.edited);
        let saved = self.auto_save(&edited);
        // Saving may trim or add a newline: that's not an edit to save again.
        self.delayed_save.versions = Some(self.docs.iter().map(|d| (d.view.id, d.editor.version())).collect());
        if saved > 0 {
            self.status = if saved == 1 { "Auto-saved".into() } else { format!("Auto-saved {saved} files") };
        }
    }

    /// Save the tabs with these view ids that can be saved without asking.
    /// Returns how many were saved.
    fn auto_save(&mut self, edited: &HashSet<Id>) -> usize {
        self.sync_views();
        let mut saved = 0;
        for i in 0..self.docs.len() {
            let d = &self.docs[i];
            // Views of one buffer save once, through the first.
            if d.view_group.is_some() && (0..i).any(|k| self.docs[k].editor.shares_history(&d.editor)) {
                continue;
            }
            let touched = (i..self.docs.len()).any(|k| edited.contains(&self.docs[k].view.id) && (k == i || self.docs[k].editor.shares_history(&d.editor)));
            if !touched || !self.auto_savable(i) {
                continue;
            }
            // Trimming on save leaves the lines being typed on alone: the
            // space just typed before the next word must stay.
            // Whether to trim comes from the file's own settings (syntax, .editorconfig).
            let trim = self.docs[i].path.clone().is_some_and(|p| self.save_time_settings(i, &p).trim_trailing_white_space_on_save);
            if trim {
                trim_except_cursor_lines(&mut self.docs[i].editor);
            }
            self.delayed_save.trimmed = true;
            let ok = self.save(i, false);
            self.delayed_save.trimmed = false;
            saved += ok as usize;
        }
        self.sync_views();
        saved
    }

    /// Tab `i` has unsaved changes Save would write without a question.
    pub(super) fn auto_savable(&self, i: usize) -> bool {
        let d = &self.docs[i];
        let Some(path) = d.path.as_deref() else { return false };
        d.is_dirty()
            && !d.editor.read_only
            && !d.conflict
            && !d.deleted
            && d.compare.is_none()
            && d.editor.buffer.check_encodable(d.encoding).is_ok()
            // Read-only on disk, changed on disk, bytes that didn't decode.
            && self.overwrite_reasons(i, path).is_empty()
    }
}

/// Trim trailing whitespace on every line without a cursor (one undo step),
/// keeping the selections on their text.
fn trim_except_cursor_lines(e: &mut crate::editor::Editor) {
    let b = &e.buffer;
    let busy: HashSet<usize> = e.selections().iter().map(|s| b.line_of(s.head)).collect();
    let edits: Vec<(std::ops::Range<usize>, String)> = (0..b.len_lines())
        .filter(|l| !busy.contains(l))
        .filter_map(|l| {
            let end = b.line_end(l);
            let start = b.line_start(l) + b.line_text(l).trim_end().chars().count();
            (start < end).then(|| (start..end, String::new()))
        })
        .collect();
    if edits.is_empty() {
        return;
    }
    // Chars removed before `p`.
    let shift = |p: usize| p - edits.iter().map(|(r, _)| r.end.min(p).saturating_sub(r.start)).sum::<usize>();
    let primary = e.selections().iter().position(|s| *s == e.primary()).unwrap_or(0);
    let sels: Vec<std::ops::Range<usize>> = e.selections().iter().map(|s| shift(s.anchor)..shift(s.head)).collect();
    e.replace_ranges(edits);
    e.select_ranges(&sels, primary);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("strata-delayed-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn app_on(path: &std::path::Path, delay: u64) -> StrataApp {
        let mut app = StrataApp::new(Some(path.to_owned()), None);
        app.settings.auto_save_delay_ms = delay;
        app.delayed_save_tick(None, 0.0);
        app
    }

    #[test]
    fn saves_once_typing_pauses() {
        let dir = scratch("pause");
        let path = dir.join("a.txt");
        std::fs::write(&path, "one\n").unwrap();
        let mut app = app_on(&path, 1000);
        app.docs[0].editor.insert_text("x");
        app.delayed_save_tick(None, 10.0);
        app.delayed_save_tick(None, 10.6);
        app.docs[0].editor.insert_text("y");
        app.delayed_save_tick(None, 10.8);
        app.delayed_save_tick(None, 11.5);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one\n", "still typing: not yet");
        app.delayed_save_tick(None, 11.85);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "xyone\n");
        assert!(!app.docs[0].is_dirty());
        assert_eq!(app.status, "Auto-saved");
        // Nothing new: nothing more happens.
        app.status.clear();
        app.delayed_save_tick(None, 30.0);
        assert_eq!(app.status, "");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn off_at_zero_and_quiet_about_restored_text() {
        let dir = scratch("off");
        let path = dir.join("a.txt");
        std::fs::write(&path, "one\n").unwrap();
        let mut app = app_on(&path, 0);
        app.docs[0].editor.insert_text("x");
        app.delayed_save_tick(None, 1.0);
        app.delayed_save_tick(None, 100.0);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one\n");
        // Unsaved text present at the first look (hot exit) isn't saved unasked.
        let mut app = StrataApp::new(Some(path.clone()), None);
        app.docs[0].editor.insert_text("restored ");
        app.settings.auto_save_delay_ms = 10;
        app.delayed_save_tick(None, 0.0);
        app.delayed_save_tick(None, 50.0);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one\n");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn respects_the_save_options() {
        let dir = scratch("opts");
        let path = dir.join("a.txt");
        std::fs::write(&path, "one  \ntwo").unwrap();
        let mut app = app_on(&path, 500);
        app.settings.trim_trailing_white_space_on_save = true;
        app.settings.ensure_newline_at_eof_on_save = true;
        let end = app.docs[0].editor.buffer.len_chars();
        app.docs[0].editor.select_ranges(&[end..end], 0);
        app.docs[0].editor.insert_text(" ");
        app.delayed_save_tick(None, 1.0);
        app.delayed_save_tick(None, 2.0);
        // The space just typed (before the next word) stays; other lines are trimmed.
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one\ntwo \n");
        assert!(app.settings.trim_trailing_white_space_on_save, "the setting is left as it was");
        // The trim and newline it made aren't an edit to save again.
        app.status.clear();
        app.delayed_save_tick(None, 3.0);
        app.delayed_save_tick(None, 4.0);
        assert_eq!(app.status, "");
        app.docs[0].editor.insert_text("more");
        assert_eq!(app.docs[0].editor.buffer.line_text(1), "two more");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn never_saves_what_save_would_ask_about() {
        let dir = scratch("never");
        let path = dir.join("a.txt");
        std::fs::write(&path, "one\n").unwrap();
        let try_save = |app: &mut StrataApp, t: f64| {
            app.docs[app.active].editor.insert_text("x");
            app.delayed_save_tick(None, t);
            app.delayed_save_tick(None, t + 5.0);
        };

        // Untitled.
        let mut app = StrataApp::new(None, None);
        app.settings.auto_save_delay_ms = 100;
        app.delayed_save_tick(None, 0.0);
        try_save(&mut app, 1.0);
        assert!(app.docs[0].is_dirty() && app.docs[0].path.is_none());

        // Read-only tab.
        let mut app = app_on(&path, 100);
        app.docs[0].editor.insert_text("x");
        app.docs[0].editor.read_only = true;
        app.delayed_save_tick(None, 1.0);
        app.delayed_save_tick(None, 9.0);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one\n");

        // Changed on disk while edited (a conflict).
        let mut app = app_on(&path, 100);
        app.docs[0].conflict = true;
        try_save(&mut app, 1.0);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one\n");

        // Changed on disk, not yet noticed.
        let mut app = app_on(&path, 100);
        app.docs[0].disk_mtime = Some(std::time::SystemTime::UNIX_EPOCH);
        try_save(&mut app, 1.0);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one\n");

        // Bytes that didn't decode.
        let mut app = app_on(&path, 100);
        app.docs[0].editor.buffer.lossy = true;
        try_save(&mut app, 1.0);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one\n");

        // Text the encoding can't hold.
        let mut app = app_on(&path, 100);
        app.docs[0].encoding = crate::encoding::Encoding::WINDOWS_1252;
        app.docs[0].editor.insert_text("漢");
        try_save(&mut app, 1.0);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one\n");

        // A file that's fine is saved.
        let mut app = app_on(&path, 100);
        try_save(&mut app, 1.0);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "xone\n");
        std::fs::remove_dir_all(dir).ok();
    }
}
