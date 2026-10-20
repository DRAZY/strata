//! Compare, the app half: opening a Compare tab (with another tab, a file,
//! or the git HEAD version), the header buttons, Next/Previous Difference,
//! and keeping a fixed side that comes from an open tab in step with it.
//! The tab's editable side is another view of the file's buffer (see
//! `clones`), so edits there are edits to the file's own tab.

use super::StrataApp;
use super::more::Choosing;
use crate::compare::{Compare, HeaderAction};
use crate::document::Document;
use crate::editor::Editor;
use crate::highlight::Highlighter;
use crate::view::EditorView;
use egui::Id;
use std::path::{Path, PathBuf};

/// What can go on the fixed (left) side.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum CompareChoice {
    /// Another tab, by view id: the left side follows its text.
    Tab(Id),
    /// A file picked in a dialog.
    File,
    /// The file as last committed (git HEAD).
    Head,
}

/// Draw a Compare tab in place of its editor view.
pub(super) fn show(c: &mut Compare, ui: &mut egui::Ui, ed: &mut Editor, hl: &mut Highlighter, view: &mut EditorView) -> egui::Response {
    if view.take_focus_request() {
        c.focus();
    }
    c.show(ui, ed, hl, view.id, view.font_size)
}

/// Header label for a file, with its folder when both sides have the same name.
fn label(name: &str, path: Option<&Path>, other: &str) -> String {
    match path.and_then(|p| p.parent()).and_then(|p| p.file_name()) {
        Some(dir) if name == other => format!("{}/{name}", dir.to_string_lossy()),
        _ => name.to_owned(),
    }
}

impl StrataApp {
    /// File ▸ Compare with… for tab `i`: pick the other side from the open
    /// tabs, a file, or HEAD. Tab `i` becomes the editable side.
    pub(super) fn compare_with(&mut self, ctx: &egui::Context, i: usize) {
        let right = self.docs[i].view.id;
        let names = self.display_names();
        let mut choices = Vec::new();
        let mut labels = Vec::new();
        for (k, d) in self.docs.iter().enumerate() {
            if k == i || d.compare.is_some() || d.editor.shares_history(&self.docs[i].editor) || d.is_blank() {
                continue;
            }
            choices.push(CompareChoice::Tab(d.view.id));
            labels.push(names[k].clone());
        }
        if self.docs[i].path.as_deref().is_some_and(|p| self.git.repo(p).is_some()) {
            choices.push(CompareChoice::Head);
            labels.push("Git: the version at HEAD".into());
        }
        choices.push(CompareChoice::File);
        labels.push("Choose a File…".into());
        if choices.len() == 1 {
            return self.compare_chosen(CompareChoice::File, right);
        }
        let hint = format!("Compare {} with…", self.docs[i].name());
        self.choose(ctx, Choosing::Compare(choices, right), &hint, labels);
    }

    /// The palette choice for Compare with… (or straight from a menu).
    pub(super) fn compare_chosen(&mut self, choice: CompareChoice, right: Id) {
        let Some(r) = self.docs.iter().position(|d| d.view.id == right) else { return };
        match choice {
            CompareChoice::Tab(id) => {
                let Some(l) = self.docs.iter().position(|d| d.view.id == id) else { return };
                let text = self.docs[l].editor.text_snapshot();
                let (name, path, syntax) = (self.docs[l].name(), self.docs[l].path.clone(), self.docs[l].highlighter.syntax_name().to_owned());
                let version = self.docs[l].editor.version();
                self.open_compare(r, &text, name, path.as_deref(), &syntax, Some((id, version)));
            }
            CompareChoice::Head => self.compare_with_head(r),
            CompareChoice::File => {
                let mut dialog = rfd::FileDialog::new();
                if let Some(dir) = self.docs[r].path.as_ref().and_then(|p| p.parent()) {
                    dialog = dialog.set_directory(dir);
                }
                if let Some(p) = dialog.pick_file() {
                    self.compare_with_file(r, &p);
                }
            }
        }
    }

    /// Compare tab `r` with the file at `path` as read from disk.
    pub(super) fn compare_with_file(&mut self, r: usize, path: &Path) {
        let text = match std::fs::read(path) {
            Ok(bytes) => {
                let b = crate::buffer::Buffer::decode(&bytes).0;
                b.slice(0..b.len_chars())
            }
            Err(e) => {
                self.status = format!("Could not read {}: {e}", path.display());
                return;
            }
        };
        let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
        let syntax = crate::highlight::syntax_for_path(path).unwrap_or_else(|| self.docs[r].highlighter.syntax_name().to_owned());
        self.open_compare(r, &text, name, Some(path), &syntax, None);
    }

    /// Git ▸ Compare with HEAD for tab `r`.
    pub(super) fn compare_with_head(&mut self, r: usize) {
        let Some(path) = self.docs[r].path.clone() else {
            self.status = "Compare with HEAD needs a saved file".into();
            return;
        };
        if self.git.repo(&path).is_none() && crate::git::repo_info(&path).is_none() {
            self.status = format!("{} isn't in a git repository", path.display());
            return;
        }
        let Some(head) = crate::git::head_text(&path) else {
            self.status = format!("{} isn't committed yet (no version at HEAD)", path.display());
            return;
        };
        let name = format!("{} (HEAD)", self.docs[r].name());
        let syntax = self.docs[r].highlighter.syntax_name().to_owned();
        self.open_compare(r, &head, name, None, &syntax, None);
    }

    /// Side bar ▸ Compare with Open File… on `path`: the file on the
    /// editable side, an open tab on the other.
    pub(super) fn compare_file_with_tab(&mut self, ctx: &egui::Context, path: PathBuf) {
        let Some(r) = self.tab_for_path(&path).or_else(|| {
            self.open_path(path.clone(), false);
            self.docs.iter().position(|d| d.path.as_ref() == Some(&path))
        }) else {
            return;
        };
        self.compare_with(ctx, r);
    }

    /// Add a Compare tab: `base` on the fixed side, a new view of tab `r` editable.
    pub(super) fn open_compare(&mut self, r: usize, base: &str, base_name: String, base_path: Option<&Path>, syntax: &str, live: Option<(Id, u64)>) {
        let right_name = self.docs[r].compare.as_ref().map_or_else(|| self.docs[r].name(), |c| c.right_name.clone());
        let base_label = label(&base_name, base_path, &right_name);
        let right_label = label(&right_name, self.docs[r].path.as_deref(), &base_name);
        let mut doc: Document = self.new_view_of(r);
        let mut c = Compare::new(base, base_label, syntax, right_label);
        if let Some((id, version)) = live {
            c.base_view = Some(id);
            c.base_version = version;
        }
        doc.compare = Some(Box::new(c));
        let group = self.docs[r].group;
        self.focus_group = group;
        let (tab, spaces) = (doc.editor.tab_width, doc.editor.use_spaces);
        self.push_doc(doc);
        let e = &mut self.docs[self.active].editor;
        (e.tab_width, e.use_spaces) = (tab, spaces);
        let d = &mut self.docs[self.active];
        if let Some(c) = d.compare.as_mut() {
            c.refresh(&mut d.editor, 0.0, true);
            let (n, removed, added) = c.stats();
            self.status = if n == 0 { "The two sides are the same".to_owned() } else { format!("{n} differences: {removed} lines removed, {added} added") };
        }
    }

    /// After drawing: header buttons, and fixed sides that follow a tab.
    pub(super) fn compare_actions(&mut self, ctx: &egui::Context) {
        for i in 0..self.docs.len() {
            let Some(c) = self.docs[i].compare.as_mut() else { continue };
            let actions = std::mem::take(&mut c.actions);
            for a in actions {
                match a {
                    HeaderAction::Next => self.step_difference_in(i, true),
                    HeaderAction::Prev => self.step_difference_in(i, false),
                    HeaderAction::Swap => self.docs[i].compare.as_mut().expect("compare tab").swap(),
                    HeaderAction::IgnoreWhitespace => self.docs[i].compare.as_mut().expect("compare tab").toggle_ignore_whitespace(),
                    HeaderAction::Keep => self.review_keep_change(i),
                    HeaderAction::Revert => self.review_revert_change(i),
                    HeaderAction::KeepFile => self.review_keep_file(i),
                    HeaderAction::RevertFile => self.review_revert_file(i),
                    // These open and close tabs: the indices here are stale after them.
                    HeaderAction::Reviewed | HeaderAction::NextFile => {
                        self.activate(i);
                        if a == HeaderAction::Reviewed {
                            self.review_mark_current();
                        } else {
                            self.review_step_file(true, false);
                        }
                        ctx.request_repaint();
                        return;
                    }
                }
                ctx.request_repaint();
            }
            let Some(id) = self.docs[i].compare.as_ref().and_then(|c| c.base_view) else { continue };
            match self.docs.iter().position(|d| d.view.id == id) {
                Some(k) if self.docs[k].editor.version() != self.docs[i].compare.as_ref().map_or(0, |c| c.base_version) => {
                    let (text, version) = (self.docs[k].editor.text_snapshot(), self.docs[k].editor.version());
                    let c = self.docs[i].compare.as_mut().expect("compare tab");
                    c.set_base(&text);
                    c.base_version = version;
                    ctx.request_repaint();
                }
                Some(_) => {}
                // The tab closed: keep its last text.
                None => self.docs[i].compare.as_mut().expect("compare tab").base_view = None,
            }
        }
    }

    /// Next/Previous Difference: in a Compare tab, its changed blocks;
    /// elsewhere, the lines changed since the last commit.
    pub(super) fn step_difference(&mut self, forward: bool) {
        if self.docs[self.active].compare.is_some() {
            return self.step_difference_in(self.active, forward);
        }
        let d = &self.docs[self.active];
        let line = d.editor.buffer.line_of(d.editor.primary().head);
        match crate::git::next_change(&d.git_changes, line, forward) {
            Some(l) => {
                self.push_jump();
                self.goto_line(l);
            }
            None => self.status = "No differences: open a Compare tab (File ▸ Compare with…) or edit a file in a git repository".into(),
        }
    }

    pub(super) fn step_difference_in(&mut self, i: usize, forward: bool) {
        let d = &mut self.docs[i];
        let Some(c) = d.compare.as_mut() else { return };
        c.refresh(&mut d.editor, 0.0, true);
        self.status = match c.step(&mut d.editor, forward) {
            Some((k, n)) => format!("Difference {k} of {n}"),
            None => "No differences".into(),
        };
        c.focus();
    }

    /// Compare ▸ Swap Sides / Ignore Whitespace for the current tab.
    pub(super) fn compare_toggle(&mut self, swap: bool) {
        match self.docs[self.active].compare.as_mut() {
            Some(c) if swap => c.swap(),
            Some(c) => {
                c.toggle_ignore_whitespace();
                self.status = format!("Whitespace is {} when comparing", if c.ignore_ws { "ignored" } else { "compared" });
            }
            None => self.status = "Not a Compare tab: use File ▸ Compare with… first".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("strata-compare-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn text(app: &StrataApp, i: usize) -> String {
        let b = &app.docs[i].editor.buffer;
        b.slice(0..b.len_chars())
    }

    #[test]
    fn compare_two_tabs_edit_the_right_and_save() {
        let dir = temp_dir("tabs");
        let (a, b) = (dir.join("old.rs"), dir.join("new.rs"));
        std::fs::write(&a, "fn main() {\n    one();\n    two();\n}\n").unwrap();
        std::fs::write(&b, "fn main() {\n    one();\n    three();\n}\n").unwrap();
        let mut app = StrataApp::new(Some(a.clone()), None);
        app.open_path(b.clone(), false);
        let (left, right) = (app.docs[0].view.id, app.docs[1].view.id);
        app.compare_chosen(CompareChoice::Tab(left), right);
        assert_eq!(app.docs.len(), 3);
        let c = app.active;
        assert_eq!(app.docs[c].name(), "old.rs ↔ new.rs");
        assert_eq!(app.docs[c].compare.as_ref().unwrap().stats(), (1, 1, 1));

        // Next Difference puts the caret on the changed line of the editable side.
        app.step_difference(true);
        assert_eq!(app.docs[c].editor.cursor_line_col(), (3, 1));
        assert_eq!(app.status, "Difference 1 of 1");

        // Editing there edits new.rs's own tab; saving saves the file.
        let at = app.docs[c].editor.buffer.line_start(2) + 4;
        app.docs[c].editor.select_ranges(&[at..at + 5], 0);
        app.docs[c].editor.insert_text("two");
        app.sync_views();
        assert_eq!(text(&app, 1), "fn main() {\n    one();\n    two();\n}\n");
        let d = &mut app.docs[c];
        d.compare.as_mut().unwrap().refresh(&mut d.editor, 0.0, true);
        assert_eq!(d.compare.as_ref().unwrap().stats(), (0, 0, 0));
        assert!(app.save(c, false));
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "fn main() {\n    one();\n    two();\n}\n");

        // The fixed side follows edits in its tab.
        app.docs[0].editor.insert_text("// ");
        app.compare_actions(&egui::Context::default());
        assert!(app.docs[c].compare.as_ref().unwrap().base_text().starts_with("// fn main"));

        // Closing the Compare tab doesn't ask (new.rs's tab keeps the buffer),
        // and hot exit doesn't keep it.
        let mut backups = Vec::new();
        assert_eq!(app.window_state(&mut backups).tabs.len(), 2);
        assert!(app.close_tab(c));
        assert_eq!(app.docs.len(), 2);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn compare_with_a_file_and_the_buttons() {
        let dir = temp_dir("file");
        let (a, b) = (dir.join("a.txt"), dir.join("b.txt"));
        std::fs::write(&a, "x\ny\n").unwrap();
        std::fs::write(&b, "x\n  y\nz\n").unwrap();
        let mut app = StrataApp::new(Some(a.clone()), None);
        app.compare_with_file(0, &b);
        let c = app.active;
        assert_eq!(app.docs[c].compare.as_ref().unwrap().stats(), (1, 2, 1));
        app.compare_toggle(false);
        let d = &mut app.docs[c];
        d.compare.as_mut().unwrap().refresh(&mut d.editor, 0.0, true);
        assert_eq!(d.compare.as_ref().unwrap().stats(), (1, 1, 0), "only z differs when whitespace is ignored");
        app.compare_toggle(true);
        assert!(app.docs[c].compare.as_ref().unwrap().swapped);
        // A file open only in a Compare tab gets a plain tab when opened.
        app.close_tab(0);
        assert_eq!(app.docs.len(), 1);
        let n = app.docs.len();
        app.open_path(a.clone(), false);
        assert_eq!(app.docs.len(), n + 1);
        assert!(app.docs[app.active].compare.is_none());
        assert!(app.docs[app.active].editor.shares_history(&app.docs[0].editor));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn untracked_files_have_no_head() {
        let dir = temp_dir("head");
        let p = dir.join("n.txt");
        std::fs::write(&p, "n\n").unwrap();
        let mut app = StrataApp::new(Some(p), None);
        app.compare_with_head(0);
        assert_eq!(app.docs.len(), 1);
        assert!(app.status.contains("git") || app.status.contains("HEAD"), "{}", app.status);
        std::fs::remove_dir_all(dir).ok();
    }
}
