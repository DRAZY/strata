//! Paths from the command line, another `strata`, or Finder; Install Shell Command.

use super::StrataApp;
use crate::cli::Target;
use std::path::Path;

impl StrataApp {
    /// Open files in tabs (at `line:col` when given) and folders in the sidebar.
    /// A file that doesn't exist yet opens empty and is created on first save.
    pub(crate) fn open_targets(&mut self, targets: &[Target]) {
        for t in targets {
            if t.path.is_dir() {
                self.open_folder(t.path.clone());
            } else if let Some(line) = t.line {
                self.open_path(t.path.clone(), false);
                if !self.has_open_file(&t.path) {
                    continue;
                }
                // Past the end means the last line.
                let line = (line - 1).min(self.docs[self.active].editor.buffer.len_lines() - 1);
                let col = t.col.unwrap_or(1) - 1;
                self.open_at(t.path.clone(), line, col..col);
            } else {
                self.open_path(t.path.clone(), false);
            }
        }
    }

    /// Is `path` open in a tab?
    pub(crate) fn has_open_file(&self, path: &Path) -> bool {
        self.docs.iter().any(|d| d.path.as_deref() == Some(path))
    }

    pub(super) fn install_shell_command(&mut self) {
        self.status = match crate::shell_command::install() {
            Ok(msg) | Err(msg) => msg,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_open_files_at_a_position_new_files_and_folders() {
        let d = std::env::temp_dir().join(format!("strata-targets-{}", std::process::id()));
        std::fs::create_dir_all(d.join("proj")).unwrap();
        std::fs::write(d.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        let mut app = StrataApp::new(None, None);
        app.open_targets(&[
            Target { path: d.join("a.txt"), line: Some(2), col: Some(3) },
            // Past the end (here: a file that doesn't exist yet) goes to the last line.
            Target { path: d.join("new.txt"), line: Some(30), col: Some(9) },
            Target::plain(d.join("proj")),
        ]);
        assert!(app.has_open_file(&d.join("a.txt")));
        // Not on disk yet: an empty tab that will be saved there.
        assert!(app.has_open_file(&d.join("new.txt")));
        assert!(!d.join("new.txt").exists());
        assert_eq!(app.sidebar.root.as_deref(), Some(d.join("proj").as_path()));
        let i = app.docs.iter().position(|doc| doc.path.as_deref() == Some(d.join("a.txt").as_path())).unwrap();
        assert_eq!(app.docs[i].editor.cursor_line_col(), (2, 3));
        let i = app.docs.iter().position(|doc| doc.path.as_deref() == Some(d.join("new.txt").as_path())).unwrap();
        assert_eq!(app.docs[i].editor.cursor_line_col(), (1, 1));
        std::fs::remove_dir_all(d).ok();
    }
}
