//! Replace in Files, the app half: open tabs are edited in their buffers
//! (one undo step each, left unsaved, like Sublime); every other file goes
//! to the sidebar's background disk job (see `crate::replace`).

use super::StrataApp;
use crate::replace::{Job, Replacer};
use crate::sidebar::OpenFile;
use std::path::{Path, PathBuf};

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned())
}

impl StrataApp {
    /// What the sidebar needs to know about the tabs: names for the Open
    /// Files list, paths for `<open files>`, unsaved text to search.
    pub(super) fn open_files_for_sidebar(&mut self) -> Vec<OpenFile> {
        let names = self.display_names();
        let active = self.active;
        self.docs
            .iter_mut()
            .zip(names)
            .enumerate()
            .map(|(i, (d, name))| {
                let dirty = d.is_dirty();
                let unsaved = (dirty && d.path.is_some()).then(|| d.editor.text_snapshot());
                OpenFile { name, dirty, active: i == active, path: d.path.clone(), unsaved }
            })
            .collect()
    }

    /// A one-line selection seeds the Find field (Find/Replace in Files).
    pub(super) fn selection_for_find_in_files(&self) -> Option<String> {
        let d = &self.docs[self.active];
        let selected = d.editor.buffer.slice(d.editor.primary().range());
        (!selected.is_empty() && !selected.contains('\n')).then_some(selected)
    }

    /// The user confirmed the replace for the sidebar's current results.
    pub(super) fn replace_in_files(&mut self, ctx: Option<egui::Context>) {
        let Some((query, _)) = self.sidebar.search.searched.clone() else { return };
        let r = match Replacer::new(&query, &self.sidebar.replacement) {
            Ok(r) => r,
            Err(e) => {
                self.status = e;
                return;
            }
        };
        let open: Vec<(usize, PathBuf)> = self.docs.iter().enumerate().filter_map(|(i, d)| Some((i, canonical(d.path.as_deref()?)))).collect();
        let (mut count, mut files) = (0, 0);
        let mut jobs = Vec::new();
        for f in &self.sidebar.search.results {
            let key = canonical(&f.path);
            match open.iter().find(|(_, p)| *p == key) {
                Some(&(i, _)) => {
                    let d = &mut self.docs[i];
                    let text = d.editor.text_snapshot();
                    let edits = r.char_edits(&text, d.editor.buffer.line_ending().as_str());
                    if !edits.is_empty() {
                        count += edits.len();
                        files += 1;
                        d.editor.replace_ranges(edits);
                        d.preview = false;
                    }
                }
                None => jobs.push(Job { path: f.path.clone(), stamp: f.stamp, expected: f.hits.len() }),
            }
        }
        self.sidebar.start_disk_replace(jobs, r, (count, files), ctx);
    }

    /// Dev aid for STRATA_DEV_FIND_IN_FILES (pair with STRATA_SCREENSHOT):
    /// search the open folder for `q`, with STRATA_DEV_WHERE in Where and
    /// STRATA_DEV_REPLACE in Replace. STRATA_DEV_REPLACE_STEP=confirm shows
    /// the confirmation; =run replaces and shows the summary.
    pub(super) fn dev_find_in_files(&mut self, q: String) {
        let env = |k: &str| crate::dev::var(k).unwrap_or_default();
        self.sidebar.start_find_in_files(Some(q));
        self.sidebar.where_text = env("STRATA_DEV_WHERE");
        self.sidebar.replacement = env("STRATA_DEV_REPLACE");
        let open = self.open_files_for_sidebar();
        match env("STRATA_DEV_REPLACE_STEP").as_str() {
            "confirm" => {
                self.sidebar.request_replace(&open, None);
                self.sidebar.search.wait();
                // The next frame's sidebar shows the confirmation.
            }
            // Replaces on disk: debug builds only (see `crate::dev`).
            "run" if crate::dev::risky_var("STRATA_DEV_REPLACE_STEP").is_some() => {
                self.sidebar.request_replace(&open, None);
                self.sidebar.search.wait();
                self.sidebar.skip_confirm();
                self.replace_in_files(None);
                self.sidebar.disk.wait_until_done();
                let open = self.open_files_for_sidebar();
                if let Some(s) = self.sidebar.poll_replace(&open, None) {
                    self.status = s;
                }
                self.sidebar.search.wait();
            }
            _ => {
                self.sidebar.run_search(&open, None);
                self.sidebar.search.wait();
                if let Some(f) = self.sidebar.search.results.first() {
                    let (p, h) = (f.path.clone(), f.hits[0].clone());
                    self.open_at(p, h.line, h.cols);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::find::Query;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("strata-app-rif-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn text(app: &StrataApp, i: usize) -> String {
        let b = &app.docs[i].editor.buffer;
        b.slice(0..b.len_chars())
    }

    #[test]
    fn open_files_change_in_their_buffer_and_the_rest_on_disk() {
        let dir = temp_dir("mixed");
        let (open_p, closed_p) = (dir.join("open.txt"), dir.join("closed.txt"));
        std::fs::write(&open_p, "foo one\nfoo two\n").unwrap();
        std::fs::write(&closed_p, "foo three\r\n").unwrap();
        let mut app = StrataApp::new(Some(dir.clone()), None);
        app.open_path(open_p.clone(), false);
        let i = app.docs.iter().position(|d| d.path.as_ref() == Some(&open_p)).unwrap();
        // An unsaved edit in the tab is searched and replaced too.
        app.docs[i].editor.select_ranges(&[0..0], 0);
        app.docs[i].editor.insert_text("foo zero\n");

        app.sidebar.query = Query::literal("foo", false);
        app.sidebar.replacement = "bar".into();
        let open = app.open_files_for_sidebar();
        app.sidebar.request_replace(&open, None);
        app.sidebar.search.wait();
        assert_eq!(app.sidebar.search.total_hits(), 4);
        app.replace_in_files(None);
        app.sidebar.disk.wait_until_done();
        let open = app.open_files_for_sidebar();
        let summary = app.sidebar.poll_replace(&open, None).unwrap();
        assert_eq!(summary, "Replaced 4 occurrences in 2 files (1 file in open tabs, unsaved)");

        // The tab: edited, unsaved, one undo step; the disk copy untouched.
        assert_eq!(text(&app, i), "bar zero\nbar one\nbar two\n");
        assert!(app.docs[i].is_dirty());
        assert_eq!(std::fs::read_to_string(&open_p).unwrap(), "foo one\nfoo two\n");
        app.docs[i].editor.undo();
        assert_eq!(text(&app, i), "foo zero\nfoo one\nfoo two\n");
        // The closed file: rewritten, CRLF kept; and Undo Replace restores it.
        assert_eq!(std::fs::read_to_string(&closed_p).unwrap(), "bar three\r\n");
        app.sidebar.undo_replace(None);
        app.sidebar.disk.wait_until_done();
        assert!(app.sidebar.poll_replace(&open, None).unwrap().contains("restored 1 file"));
        assert_eq!(std::fs::read_to_string(&closed_p).unwrap(), "foo three\r\n");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn sidebar_with_replace_fields_renders_headlessly() {
        let dir = temp_dir("ui");
        std::fs::write(dir.join("a.txt"), "foo\n").unwrap();
        let mut app = StrataApp::new(Some(dir.clone()), None);
        app.sidebar.start_replace_in_files(Some("foo".into()));
        let open = app.open_files_for_sidebar();
        app.sidebar.request_replace(&open, None);
        app.sidebar.search.wait();
        let ctx = egui::Context::default();
        for _ in 0..2 {
            let raw = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(300.0, 400.0))), ..Default::default() };
            let mut out = ctx.run_ui(raw, |ui| {
                app.sidebar.show(ui, None, &open);
            });
            out.textures_delta.clear();
        }
        assert!(app.sidebar.confirming(), "the confirmation is up after the search");
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "foo\n", "nothing changes before confirming");
        std::fs::remove_dir_all(dir).ok();
    }
}
