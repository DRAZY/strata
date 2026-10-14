//! Several tabs on one buffer (File ▸ New View into File, the tab menu's
//! Split View, and the editable side of a Compare tab).
//!
//! Each view is a full `Document` with its own `Editor`, so selections,
//! scroll, folds, soft undo, find state and word wrap stay per view. What
//! must be one thing is shared or mirrored:
//! - the undo history and saved point live in one `Rc` the editors share
//!   (`Editor::new_view`), so undo works from any view and "unsaved" is one
//!   state;
//! - every text change is journaled by the view that made it and replayed
//!   into the others (`sync_views`, run at the start and middle of each
//!   frame), moving their selections, bookmarks and folds with the text;
//! - file state (path, encoding, disk time, conflict, line endings) is
//!   copied from whichever view changed it, and a view whose editor was
//!   replaced (Revert, Reload, Reopen with Encoding) hands its text to the
//!   others.

use super::StrataApp;
use crate::buffer::{Buffer, LineEnding};
use crate::document::Document;
use crate::encoding::Encoding;
use crate::panes::Layout;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// File state every view of a buffer shows.
#[derive(Clone, PartialEq, Debug)]
pub(super) struct Meta {
    path: Option<PathBuf>,
    encoding: Encoding,
    restored_dirty: bool,
    disk_mtime: Option<SystemTime>,
    disk_len: Option<u64>,
    conflict: bool,
    deleted: bool,
    line_ending: LineEnding,
    lossy: bool,
}

impl Meta {
    fn of(d: &Document) -> Self {
        Self {
            path: d.path.clone(),
            encoding: d.encoding,
            restored_dirty: d.restored_dirty,
            disk_mtime: d.disk_mtime,
            disk_len: d.disk_len,
            conflict: d.conflict,
            deleted: d.deleted,
            line_ending: d.editor.buffer.line_ending(),
            lossy: d.editor.buffer.lossy,
        }
    }

    fn apply(&self, d: &mut Document) {
        if d.path != self.path {
            if let Some(p) = &self.path {
                d.highlighter.detect(Some(p), &d.editor.buffer.line_text(0));
            }
            d.path = self.path.clone();
        }
        d.encoding = self.encoding;
        d.restored_dirty = self.restored_dirty;
        d.disk_mtime = self.disk_mtime;
        d.disk_len = self.disk_len;
        d.conflict = self.conflict;
        d.deleted = self.deleted;
        d.editor.buffer.set_line_ending(self.line_ending);
        d.editor.buffer.lossy = self.lossy;
    }
}

/// Per window: the file state each group of views last agreed on.
#[derive(Default)]
pub(super) struct Views {
    meta: HashMap<u64, Meta>,
}

fn next_group() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

impl StrataApp {
    /// A new tab showing tab `i`'s buffer (not yet added to the window).
    pub(super) fn new_view_of(&mut self, i: usize) -> Document {
        let group = *self.docs[i].view_group.get_or_insert_with(next_group);
        let src = &mut self.docs[i];
        let mut doc = Document::from_buffer(src.path.clone(), Buffer::from_str(""));
        doc.editor = src.editor.new_view();
        doc.highlighter.set_syntax(src.highlighter.syntax_name());
        doc.encoding = src.encoding;
        doc.restored_dirty = src.restored_dirty;
        doc.disk_mtime = src.disk_mtime;
        doc.disk_len = src.disk_len;
        doc.conflict = src.conflict;
        doc.deleted = src.deleted;
        doc.wrap = src.wrap;
        doc.title = src.title.clone();
        doc.git_changes = src.git_changes.clone();
        doc.view_group = Some(group);
        doc.view.reveal_centered();
        self.views.meta.entry(group).or_insert_with(|| Meta::of(&self.docs[i]));
        doc
    }

    /// Add `doc` (a new view) to pane `group`, keeping the indentation it
    /// was given rather than the defaults `push_doc` applies.
    fn push_view(&mut self, doc: Document, group: usize) {
        let (tab, spaces) = (doc.editor.tab_width, doc.editor.use_spaces);
        self.focus_group = group;
        self.push_doc(doc);
        let e = &mut self.docs[self.active].editor;
        (e.tab_width, e.use_spaces) = (tab, spaces);
    }

    /// File ▸ New View into File: a second tab on the current buffer, in the same pane.
    pub(super) fn clone_file(&mut self) {
        let i = self.active;
        let doc = self.new_view_of(i);
        let group = self.docs[i].group;
        self.push_view(doc, group);
        self.status = format!("New view into {}", self.docs[self.active].name());
    }

    /// Tab menu ▸ Split View: a view of tab `i` in the next pane, adding a
    /// second column when there is only one pane.
    pub(super) fn split_view(&mut self, i: usize) {
        if self.layout.groups() < 2 {
            self.set_layout(Layout::grid(2, 1));
        }
        let target = (self.docs[i].group + 1) % self.layout.groups();
        let doc = self.new_view_of(i);
        self.push_view(doc, target);
    }

    /// Dev aid (STRATA_DEV_PALETTE="splitview:120"): Split View with the new
    /// view's caret on line 120.
    pub(super) fn dev_split_view(&mut self, line: &str) {
        self.split_view(self.active);
        if let Ok(n) = line.parse::<usize>() {
            self.goto_line(n.saturating_sub(1));
        }
    }

    /// Another tab showing tab `i`'s buffer, if there is one.
    pub(super) fn other_view(&self, i: usize) -> Option<usize> {
        let d = &self.docs[i];
        d.view_group?;
        (0..self.docs.len()).find(|&k| k != i && self.docs[k].editor.shares_history(&d.editor))
    }

    /// Closing tab `i` loses nothing because a later tab shows the same
    /// buffer (window close and quit ask once per buffer, at its last view).
    pub(super) fn later_view(&self, i: usize) -> bool {
        self.other_view(i).is_some_and(|_| (i + 1..self.docs.len()).any(|k| self.docs[k].editor.shares_history(&self.docs[i].editor)))
    }

    /// The tab to show for `path`: the current tab if it is that file, else
    /// its most recently used plain view. A file open only in Compare tabs
    /// gets a plain view of their buffer (added to the window).
    pub(super) fn tab_for_path(&mut self, path: &Path) -> Option<usize> {
        let views: Vec<usize> = (0..self.docs.len()).filter(|&k| self.docs[k].path.as_deref() == Some(path)).collect();
        let plain = |k: &&usize| self.docs[**k].compare.is_none();
        if views.contains(&self.active) && self.docs[self.active].compare.is_none() {
            return Some(self.active);
        }
        if let Some(&k) = views.iter().filter(plain).max_by_key(|&&k| self.docs[k].stamp) {
            return Some(k);
        }
        let &k = views.first()?;
        let doc = self.new_view_of(k);
        self.push_view(doc, self.focus_group);
        Some(self.active)
    }

    /// How hot exit keeps tab `i`: `None` as itself; `Some(Some(k))` as
    /// another view of tab `k`'s buffer (`k < i`, which holds the backup);
    /// `Some(None)` not at all (a Compare tab whose file a plain tab holds).
    pub(super) fn session_view_of(&self, i: usize) -> Option<Option<usize>> {
        let d = &self.docs[i];
        d.view_group?;
        let members: Vec<usize> = (0..self.docs.len()).filter(|&k| self.docs[k].editor.shares_history(&d.editor)).collect();
        let main = members.iter().copied().find(|&k| self.docs[k].compare.is_none()).unwrap_or(members[0]);
        match () {
            _ if main == i => None,
            _ if d.compare.is_some() => Some(None),
            _ => Some(Some(main)),
        }
    }

    /// Before tab `i` leaves this window: it stops sharing its buffer with
    /// tabs that stay (they can't be kept in step across windows).
    pub(super) fn detach_view(doc: &mut Document) {
        if doc.view_group.take().is_some() && doc.editor.view_count() > 1 && doc.editor.detach_history() {
            doc.restored_dirty = true;
        }
    }

    /// Keep every view of a buffer in step: replay edits, adopt replaced
    /// editors, copy changed file state. Returns true if anything changed.
    pub(super) fn sync_views(&mut self) -> bool {
        let mut groups: Vec<(u64, Vec<usize>)> = Vec::new();
        for (i, d) in self.docs.iter().enumerate() {
            let Some(g) = d.view_group else { continue };
            match groups.iter_mut().find(|(k, _)| *k == g) {
                Some((_, v)) => v.push(i),
                None => groups.push((g, vec![i])),
            }
        }
        self.views.meta.retain(|g, _| groups.iter().any(|(k, _)| k == g));
        let mut changed = false;
        for (g, members) in groups {
            if members.len() < 2 {
                // The last view: an ordinary tab again.
                let d = &mut self.docs[members[0]];
                d.view_group = None;
                d.editor.set_journal(false);
                self.views.meta.remove(&g);
                continue;
            }
            changed |= self.sync_group(g, &members);
        }
        changed
    }

    fn sync_group(&mut self, g: u64, members: &[usize]) -> bool {
        let mut changed = false;
        let shared = |a: usize, b: usize| self.docs[a].editor.shares_history(&self.docs[b].editor);
        // An editor was replaced (reload, revert, reopen with encoding): its
        // text is the truth now. Views made by `new_view` journal their
        // edits; a replacement made by `Editor::new` doesn't.
        if !members.iter().all(|&k| shared(k, members[0])) {
            let src = members.iter().copied().find(|&k| !self.docs[k].editor.journaling()).unwrap_or(members[0]);
            for &k in members.iter().filter(|&&k| k != src) {
                self.rebuild_view(src, k);
            }
            changed = true;
        }
        let edited: Vec<(usize, Vec<crate::editor::Splice>)> =
            members.iter().filter_map(|&k| Some((k, self.docs[k].editor.take_journal())).filter(|(_, j)| !j.is_empty())).collect();
        match edited.as_slice() {
            [] => {}
            [(src, splices)] => {
                for &k in members.iter().filter(|&&k| k != *src) {
                    self.docs[k].editor.apply_foreign(splices);
                }
                changed = true;
            }
            _ => {
                // Two views edited in one frame (nothing does that on purpose):
                // the most recently used one wins.
                let src = members.iter().copied().max_by_key(|&k| self.docs[k].stamp).unwrap_or(members[0]);
                for &k in members.iter().filter(|&&k| k != src) {
                    self.rebuild_view(src, k);
                }
                changed = true;
            }
        }
        let len = self.docs[members[0]].editor.buffer.len_chars();
        if members.iter().any(|&k| self.docs[k].editor.buffer.len_chars() != len) {
            // Out of step (a bug elsewhere): resync from the current view.
            let src = members.iter().copied().max_by_key(|&k| self.docs[k].stamp).unwrap_or(members[0]);
            for &k in members.iter().filter(|&&k| k != src) {
                self.rebuild_view(src, k);
            }
            changed = true;
        }
        let metas: Vec<Meta> = members.iter().map(|&k| Meta::of(&self.docs[k])).collect();
        let src = match self.views.meta.get(&g) {
            Some(known) => metas.iter().position(|m| m != known),
            None => metas.iter().any(|m| *m != metas[0]).then_some(0),
        };
        if let Some(s) = src {
            for &k in members {
                metas[s].apply(&mut self.docs[k]);
            }
            changed = true;
        }
        self.views.meta.insert(g, metas[src.unwrap_or(0)].clone());
        changed
    }

    /// Point view `k` at view `src`'s buffer again, keeping its caret line and column.
    fn rebuild_view(&mut self, src: usize, k: usize) {
        let mut editor = self.docs[src].editor.new_view();
        let old = &self.docs[k].editor;
        let (line, col) = old.cursor_line_col();
        (editor.tab_width, editor.use_spaces, editor.read_only) = (old.tab_width, old.use_spaces, old.read_only);
        let b = &editor.buffer;
        let line = (line - 1).min(b.len_lines() - 1);
        let at = (b.line_start(line) + col - 1).min(b.line_end(line));
        editor.select_ranges(&[at..at], 0);
        let d = &mut self.docs[k];
        d.editor = editor;
        d.highlighter.invalidate_from(0);
        d.finder = crate::find::Finder::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Editor;

    fn app_with(text: &str) -> StrataApp {
        let mut app = StrataApp::new(None, None);
        app.docs[0].editor = Editor::new(Buffer::from_str(text));
        app
    }

    fn text(app: &StrataApp, i: usize) -> String {
        let b = &app.docs[i].editor.buffer;
        b.slice(0..b.len_chars())
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("strata-views-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn edits_in_one_view_show_in_the_other_and_shift_its_selections() {
        let mut app = app_with("alpha\nbeta\ngamma\n");
        app.docs[0].editor.select_ranges(&[11..16], 0); // "gamma"
        app.docs[0].editor.toggle_bookmark();
        app.clone_file();
        assert_eq!(app.docs.len(), 2);
        assert_eq!(app.active, 1);
        // The new view starts where the old one was, then goes its own way.
        assert_eq!(app.docs[1].editor.primary().range(), 11..16);
        app.docs[1].editor.select_ranges(&[0..0], 0);
        app.docs[1].editor.insert_text("// ");
        assert!(app.sync_views());
        assert_eq!(text(&app, 0), "// alpha\nbeta\ngamma\n");
        assert_eq!(app.docs[0].editor.primary().range(), 14..19, "the other view's selection moved with the text");
        assert_eq!(app.docs[0].editor.bookmarks(), [19], "and its bookmark (set at the caret, 16)");
        assert!(app.docs[0].is_dirty() && app.docs[1].is_dirty(), "one unsaved state");

        // Deleting text under a selection collapses it.
        app.docs[1].editor.select_ranges(&[12..19], 0);
        app.docs[1].editor.backspace(false);
        app.sync_views();
        assert_eq!(text(&app, 0), "// alpha\nbet\n");
        assert_eq!(app.docs[0].editor.primary().range(), 12..12);

        // Undo from the other view undoes the shared history, newest first.
        app.docs[0].editor.undo();
        app.sync_views();
        assert_eq!(text(&app, 1), "// alpha\nbeta\ngamma\n");
        app.docs[0].editor.undo();
        app.sync_views();
        assert_eq!(text(&app, 1), "alpha\nbeta\ngamma\n");
        assert!(!app.docs[0].is_dirty() && !app.docs[1].is_dirty());
        app.docs[1].editor.redo();
        app.sync_views();
        assert_eq!(text(&app, 0), "// alpha\nbeta\ngamma\n");
    }

    #[test]
    fn typing_in_two_views_makes_separate_undo_steps() {
        let mut app = app_with("x\n");
        app.clone_file();
        app.docs[0].editor.insert_text("a");
        app.sync_views();
        app.docs[1].editor.select_ranges(&[0..0], 0);
        app.docs[1].editor.insert_text("b");
        app.sync_views();
        app.docs[0].editor.insert_text("c");
        app.sync_views();
        assert_eq!(text(&app, 1), "bacx\n");
        app.docs[0].editor.undo();
        app.sync_views();
        assert_eq!(text(&app, 1), "bax\n");
        app.docs[0].editor.undo();
        app.sync_views();
        assert_eq!(text(&app, 1), "ax\n");
    }

    #[test]
    fn saving_or_closing_a_view_and_reloading() {
        let dir = temp_dir("save");
        let path = dir.join("a.txt");
        std::fs::write(&path, "one\n").unwrap();
        let mut app = StrataApp::new(Some(path.clone()), None);
        app.split_view(0);
        assert_eq!(app.layout.groups(), 2, "Split View adds a column");
        assert_eq!((app.docs[0].group, app.docs[1].group), (0, 1));
        app.docs[1].editor.insert_text("zero ");
        app.sync_views();
        assert!(app.docs[0].is_dirty());
        assert!(app.save(0, false));
        app.sync_views();
        assert!(!app.docs[1].is_dirty(), "saving one view saves the buffer");
        assert_eq!(app.docs[1].disk_mtime, app.docs[0].disk_mtime);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "zero one\n");

        // Closing a view of an unsaved buffer doesn't ask: another view keeps it.
        app.docs[0].editor.insert_text("!");
        app.sync_views();
        assert!(app.later_view(0) && !app.later_view(1));
        assert!(app.close_tab(0), "no prompt (a prompt would fail in tests)");
        assert_eq!(app.docs.len(), 1);
        assert!(app.docs[0].is_dirty());
        app.sync_views();
        assert_eq!(app.docs[0].view_group, None, "the last view is a plain tab again");

        // Reloading one view (here: revert) hands the new text to the others.
        let doc = app.new_view_of(0);
        app.push_view(doc, 0);
        std::fs::write(&path, "fresh\n").unwrap();
        app.docs[1].reload_from_disk().unwrap();
        assert!(app.sync_views());
        assert_eq!(text(&app, 0), "fresh\n");
        assert!(!app.docs[0].is_dirty());
        app.docs[0].editor.select_ranges(&[0..0], 0);
        app.docs[0].editor.insert_text("+");
        app.sync_views();
        assert_eq!(text(&app, 1), "+fresh\n", "still in step after the reload");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn opening_a_file_prefers_the_current_view() {
        let dir = temp_dir("open");
        let path = dir.join("b.txt");
        std::fs::write(&path, "b\n").unwrap();
        let mut app = StrataApp::new(Some(path.clone()), None);
        app.clone_file();
        app.activate(0);
        app.open_path(path.clone(), false);
        assert_eq!(app.active, 0);
        app.activate(1);
        app.open_path(path.clone(), false);
        assert_eq!(app.active, 1);
        assert_eq!(app.docs.len(), 2);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn hot_exit_keeps_both_views_and_backs_up_once() {
        let dir = temp_dir("session");
        let path = dir.join("c.txt");
        std::fs::write(&path, "c1\nc2\n").unwrap();
        let mut app = StrataApp::new(Some(path.clone()), None);
        app.split_view(0);
        app.docs[1].editor.insert_text("new ");
        app.docs[1].editor.select_ranges(&[4..5], 0);
        app.sync_views();
        app.docs[0].editor.select_ranges(&[9..9], 0);
        let mut backups = Vec::new();
        let state = app.window_state(&mut backups);
        assert_eq!(backups.len(), 1, "one backup for the shared buffer");
        assert_eq!(state.tabs.len(), 2);
        assert_eq!(state.tabs[1].clone_of, Some(0));
        assert!(state.tabs[1].backup.is_none());

        let config = temp_dir("session-config");
        let session = crate::session::Session::of_windows(vec![state]);
        session.save(&config, &backups).unwrap();
        let mut restored = StrataApp::with_settings(Some(config.clone()), Default::default(), String::new());
        let loaded = crate::session::Session::load(&config).unwrap().unwrap();
        restored.restore_window(&config, &loaded.window_states()[0]);
        assert_eq!(restored.docs.len(), 2);
        assert_eq!(text(&restored, 0), "new c1\nc2\n");
        assert_eq!(text(&restored, 1), "new c1\nc2\n");
        assert!(restored.docs[0].editor.shares_history(&restored.docs[1].editor));
        assert_eq!(restored.docs[0].editor.primary().range(), 9..9);
        assert_eq!(restored.docs[1].editor.primary().range(), 4..5);
        assert_eq!((restored.docs[0].group, restored.docs[1].group), (0, 1));
        restored.docs[0].editor.insert_text("x");
        restored.sync_views();
        assert_eq!(text(&restored, 1), "new c1\nc2x\n");
        std::fs::remove_dir_all(dir).ok();
        std::fs::remove_dir_all(config).ok();
    }
}
