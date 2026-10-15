//! Copy Relative Path (from the side bar folder holding the file) and Copy
//! Permalink (see `permalink`), for the palette and the tab and editor
//! right-click menus.

use super::StrataApp;
use crate::commands::Cmd;
use crate::keybindings::Target;
use crate::menus::{Act, Item};
use std::path::{Path, PathBuf};

/// Menu-only actions (`Act::Named`), for the tab they were opened on.
pub(super) const COPY_RELATIVE: &str = "ctx:copy_relative_path";
pub(super) const COPY_PERMALINK: &str = "ctx:copy_permalink";

/// `path` relative to the deepest of `roots` holding it.
pub fn relative_path(roots: &[PathBuf], path: &Path) -> Option<String> {
    let root = roots.iter().filter(|r| path.starts_with(r)).max_by_key(|r| r.components().count())?;
    let rel = path.strip_prefix(root).ok()?;
    (!rel.as_os_str().is_empty()).then(|| rel.display().to_string())
}

impl StrataApp {
    /// Copy Relative Path and Copy Permalink, for a menu about tab `i`.
    pub(super) fn file_link_items(&self, i: usize) -> Vec<Item> {
        let path = self.docs[i].path.as_deref();
        let relative = path.is_some_and(|p| relative_path(&self.sidebar.roots, p).is_some());
        // Known to be in a repository (git's cached answer), with a remote to link to.
        let linkable = path.and_then(|p| self.git.repo(p).map(|_| p)).and_then(crate::git::work_dir).and_then(crate::permalink::remote_for).is_some();
        vec![
            Item::new("Copy Relative Path", Act::Named(COPY_RELATIVE.into())).bound(Target::Cmd(Cmd::CopyRelativePath)).enabled(relative),
            Item::new("Copy Permalink", Act::Named(COPY_PERMALINK.into())).bound(Target::Cmd(Cmd::CopyPermalink)).enabled(linkable),
        ]
    }

    pub(super) fn copy_relative_path(&mut self, ctx: &egui::Context, i: usize) {
        let Some(path) = self.docs.get(i).and_then(|d| d.path.clone()) else {
            self.status = "Copy Relative Path: the file hasn't been saved yet".into();
            return;
        };
        match relative_path(&self.sidebar.roots, &path) {
            Some(rel) => {
                ctx.copy_text(rel.clone());
                self.status = format!("Copied {rel}");
            }
            None => self.status = "Copy Relative Path: the file isn't in a folder open in the side bar".into(),
        }
    }

    /// The link to tab `i`'s file at the selected lines.
    pub(super) fn copy_permalink(&mut self, ctx: &egui::Context, i: usize) {
        let Some(d) = self.docs.get(i) else { return };
        let Some(path) = d.path.clone() else {
            self.status = "Copy Permalink: the file hasn't been saved yet".into();
            return;
        };
        let (first, last) = selected_lines(&d.editor);
        match crate::permalink::permalink(&path, first, last) {
            Ok(url) => {
                ctx.copy_text(url.clone());
                self.status = format!("Copied {url}");
            }
            Err(why) => self.status = why.into(),
        }
    }
}

/// The primary selection's lines, 1-based; a selection ending at the start
/// of a line doesn't include that line.
fn selected_lines(e: &crate::editor::Editor) -> (usize, usize) {
    let r = e.primary().range();
    let b = &e.buffer;
    let first = b.line_of(r.start);
    let mut last = b.line_of(r.end);
    if last > first && b.line_start(last) == r.end {
        last -= 1;
    }
    (first + 1, last + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;
    use crate::editor::Editor;

    #[test]
    fn relative_to_the_deepest_folder() {
        let roots = vec![PathBuf::from("/w/proj"), PathBuf::from("/w/proj/sub")];
        let sep = std::path::MAIN_SEPARATOR;
        assert_eq!(relative_path(&roots, Path::new("/w/proj/a/b.rs")), Some(format!("a{sep}b.rs")));
        assert_eq!(relative_path(&roots, Path::new("/w/proj/sub/c.rs")), Some("c.rs".into()));
        assert_eq!(relative_path(&roots, Path::new("/w/project/c.rs")), None, "a name that only starts the same isn't inside");
        assert_eq!(relative_path(&roots, Path::new("/w/proj")), None);
    }

    #[test]
    fn lines_of_the_selection() {
        let mut e = Editor::new(Buffer::from_str("a\nb\nc\nd\n"));
        e.select_ranges(&[2..2], 0);
        assert_eq!(selected_lines(&e), (2, 2));
        e.select_ranges(&[2..5], 0);
        assert_eq!(selected_lines(&e), (2, 3));
        e.select_ranges(&[2..6], 0);
        assert_eq!(selected_lines(&e), (2, 3), "ending at a line start leaves that line out");
        e.select_ranges(&[std::ops::Range { start: 7, end: 2 }], 0);
        assert_eq!(selected_lines(&e), (2, 4), "either direction");
    }

    fn enabled(item: &Item) -> bool {
        matches!(item, Item::Action { enabled: true, .. })
    }

    #[test]
    fn commands_copy_or_explain() {
        let dir = std::env::temp_dir().join(format!("strata-rel-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/x.rs"), "fn x() {}\n").unwrap();
        let ctx = egui::Context::default();
        let mut app = StrataApp::new(Some(dir.clone()), None);
        app.open_path(dir.join("src/x.rs"), false);
        app.run(&ctx, Cmd::CopyRelativePath);
        assert_eq!(app.status, format!("Copied src{}x.rs", std::path::MAIN_SEPARATOR));
        assert!(enabled(&app.file_link_items(app.active)[0]));
        app.run(&ctx, Cmd::CopyPermalink);
        assert!(app.status.starts_with("Copy Permalink: "), "{}", app.status);
        assert!(!enabled(&app.file_link_items(app.active)[1]), "no repository");

        app.run(&ctx, Cmd::NewTab);
        app.run(&ctx, Cmd::CopyRelativePath);
        assert_eq!(app.status, "Copy Relative Path: the file hasn't been saved yet");
        assert!(!enabled(&app.file_link_items(app.active)[0]));
        std::fs::remove_dir_all(dir).ok();
    }
}
