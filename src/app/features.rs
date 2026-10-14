//! Window-level features behind the menus: recent files, revert, jump
//! history, go to definition, projects, distraction-free mode, About
//! (the manual is in `help_ui`).

use super::StrataApp;
use crate::panes::Layout;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const RECENT_MAX: usize = 15;
const JUMPS_MAX: usize = 100;

/// A `.strata-project` file: the folders, plus the files and layout to reopen.
#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct Project {
    folders: Vec<PathBuf>,
    files: Vec<PathBuf>,
    layout: Option<Layout>,
}

fn push_recent(list: &mut Vec<PathBuf>, p: &Path) {
    list.retain(|q| q != p);
    list.insert(0, p.to_owned());
    list.truncate(RECENT_MAX);
}

impl StrataApp {
    pub(super) fn remember_file(&mut self, p: &Path) {
        push_recent(&mut self.recent_files, p);
    }

    /// Make `dir` the window's only folder.
    pub(super) fn open_folder(&mut self, dir: PathBuf) {
        push_recent(&mut self.recent_folders, &dir);
        self.sidebar.open_folder(dir);
    }

    /// Project ▸ Add Folder to Project…: `dir` joins the window's folders.
    pub(super) fn add_folder(&mut self, dir: PathBuf) {
        push_recent(&mut self.recent_folders, &dir);
        if !self.sidebar.add_folder(dir.clone()) {
            self.status = format!("{} is already in the side bar", dir.display());
        }
    }

    pub(super) fn clear_recent(&mut self) {
        self.recent_files.clear();
        self.recent_folders.clear();
        self.recent_projects.clear();
        self.closed.clear();
    }

    /// Throw away unsaved edits and reload from disk (asks first when dirty).
    pub(super) fn revert(&mut self) {
        let d = &self.docs[self.active];
        let Some(path) = d.path.clone() else { return };
        if d.is_dirty() {
            let ok = rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Warning)
                .set_title("Revert File")
                .set_description(format!("Discard your changes to {} and reload it from disk?\n\nUndo brings your changes back.", d.name()))
                .set_buttons(rfd::MessageButtons::OkCancel)
                .show();
            if ok != rfd::MessageDialogResult::Ok {
                return;
            }
        }
        self.status = match self.doc().reload_from_disk() {
            Ok(()) => format!("Reverted {}", path.display()),
            Err(e) => format!("Could not read {}: {e}", path.display()),
        };
    }

    /// "Reopen with Encoding": re-read the file decoding it as `enc`.
    pub(super) fn reopen_with_encoding(&mut self, enc: crate::encoding::Encoding) {
        if self.docs[self.active].is_dirty() {
            self.status = "Save or revert your changes before reopening with another encoding".into();
            return;
        }
        self.status = match self.doc().reopen_with_encoding(enc) {
            Ok(()) => format!("Reopened as {}", enc.name()),
            Err(e) => format!("Could not reopen: {e}"),
        };
    }

    /// "Save with Encoding": convert and save now.
    pub(super) fn save_with_encoding(&mut self, enc: crate::encoding::Encoding) {
        let d = &self.docs[self.active];
        let previous = (d.encoding, d.restored_dirty);
        self.doc().set_encoding(enc);
        if !self.save(self.active, false) {
            // Cancelled or failed: the tab is exactly as it was, clean if it was.
            let d = self.doc();
            (d.encoding, d.restored_dirty) = previous;
        }
    }

    // ---- jump history ----------------------------------------------------------

    /// Remember where the cursor is before a jump (goto line, definition, search hit…).
    pub(super) fn push_jump(&mut self) {
        let d = &self.docs[self.active];
        let here = (d.view.id, d.editor.primary().head);
        if self.jumps_back.last() != Some(&here) {
            self.jumps_back.push(here);
            if self.jumps_back.len() > JUMPS_MAX {
                self.jumps_back.remove(0);
            }
        }
        self.jumps_forward.clear();
    }

    pub(super) fn jump(&mut self, back: bool) {
        let d = &self.docs[self.active];
        let here = (d.view.id, d.editor.primary().head);
        let (from, to) = if back { (&mut self.jumps_back, &mut self.jumps_forward) } else { (&mut self.jumps_forward, &mut self.jumps_back) };
        // Skip entries whose tab has been closed.
        while let Some((id, pos)) = from.pop() {
            if let Some(i) = self.docs.iter().position(|d| d.view.id == id) {
                to.push(here);
                self.activate(i);
                let d = self.doc();
                let pos = pos.min(d.editor.buffer.len_chars());
                d.editor.select_ranges(&[pos..pos], 0);
                d.view.reveal_centered();
                return;
            }
        }
    }

    // ---- go to definition ------------------------------------------------------------

    /// The identifier at (or touching) the primary cursor.
    pub(super) fn word_at_cursor(&self) -> Option<String> {
        let ed = &self.docs[self.active].editor;
        let p = ed.primary();
        if !p.is_empty() {
            return Some(ed.buffer.slice(p.range())).filter(|s| !s.contains('\n'));
        }
        let line = ed.buffer.line_of(p.head);
        let start = ed.buffer.line_start(line);
        let chars: Vec<char> = ed.buffer.line_text(line).chars().collect();
        let col = p.head - start;
        let is_word = |c: &char| c.is_alphanumeric() || *c == '_';
        let a = (0..col).rev().take_while(|&i| is_word(&chars[i])).last().unwrap_or(col);
        let b = (col..chars.len()).take_while(|&i| is_word(&chars[i])).last().map_or(col, |i| i + 1);
        (b > a).then(|| chars[a..b].iter().collect())
    }

    /// F12: find where the word under the cursor is defined, in this file,
    /// the other open files, then the folder (files of the same type).
    pub(super) fn goto_definition(&mut self) {
        let Some(word) = self.word_at_cursor() else { return };
        let here = &self.docs[self.active];
        let syntax = here.highlighter.syntax_name().to_owned();
        let ext = here.path.as_ref().and_then(|p| p.extension()).map(|e| e.to_owned());

        let mut order: Vec<usize> = vec![self.active];
        order.extend((0..self.docs.len()).filter(|&i| i != self.active));
        for i in order {
            let d = &mut self.docs[i];
            let text = d.editor.text_snapshot();
            if let Some(s) = crate::symbols::symbols(d.highlighter.syntax_name(), &text).into_iter().find(|s| s.name == word) {
                self.push_jump();
                self.activate(i);
                self.goto_line(s.line);
                return;
            }
        }
        if let (Some(walker), Some(ext)) = (self.walk_folders(), ext) {
            for entry in walker.flatten().take(20_000) {
                let p = entry.path();
                if p.extension() != Some(ext.as_os_str()) || entry.metadata().map_or(true, |m| m.len() > 2_000_000) {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(p) else { continue };
                if !text.contains(&word) {
                    continue;
                }
                if let Some(s) = crate::symbols::symbols(&syntax, &text).into_iter().find(|s| s.name == word) {
                    self.push_jump();
                    self.open_path(p.to_owned(), false);
                    self.goto_line(s.line);
                    return;
                }
            }
        }
        self.status = format!("No definition found for {word}");
    }

    // ---- projects ----------------------------------------------------------------

    pub(super) fn open_project(&mut self) {
        if let Some(path) = rfd::FileDialog::new().add_filter("Strata Project", &["strata-project"]).pick_file() {
            self.open_project_path(path);
        }
    }

    pub(super) fn open_project_path(&mut self, path: PathBuf) {
        let project: Project = match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string())) {
            Ok(p) => p,
            Err(e) => {
                self.status = format!("Could not open project: {e}");
                return;
            }
        };
        let base = path.parent().unwrap_or(Path::new("."));
        // The project's folders replace the window's.
        for (k, folder) in project.folders.iter().enumerate() {
            let dir = base.join(folder);
            let dir = if folder.as_os_str().is_empty() { base.to_owned() } else { dir };
            if k == 0 {
                self.open_folder(dir);
            } else {
                self.add_folder(dir);
            }
        }
        if let Some(layout) = project.layout {
            self.set_layout(layout.sanitized());
        }
        for f in project.files {
            let f = base.join(f);
            if f.exists() {
                self.open_path(f, false);
            }
        }
        self.status = format!("Opened project {}", path.display());
        self.remember_project(&path);
    }

    pub(super) fn save_project_as(&mut self) {
        let name = self.sidebar.root().and_then(|r| r.file_name()).map_or("project".into(), |n| n.to_string_lossy().into_owned());
        let mut dialog = rfd::FileDialog::new().add_filter("Strata Project", &["strata-project"]).set_file_name(format!("{name}.strata-project"));
        if let Some(root) = self.sidebar.root() {
            dialog = dialog.set_directory(root);
        }
        if let Some(path) = dialog.save_file() {
            self.save_project_to(&path);
        }
    }

    /// Write the window's folders, files and layout as a project file.
    pub(super) fn save_project_to(&mut self, path: &Path) {
        let base = path.parent().unwrap_or(Path::new("."));
        // Paths are stored relative to the project file when they're inside its folder.
        let rel = |p: &Path| p.strip_prefix(base).map(Path::to_owned).unwrap_or_else(|_| p.to_owned());
        let project = Project {
            folders: self.sidebar.roots.iter().map(|r| rel(r)).collect(),
            files: self.docs.iter().filter_map(|d| d.path.as_deref().map(rel)).collect(),
            layout: Some(self.layout.clone()),
        };
        let text = serde_json::to_string_pretty(&project).unwrap_or_default();
        self.status = match std::fs::write(path, text + "\n") {
            Ok(()) => {
                self.remember_project(path);
                format!("Saved project {}", path.display())
            }
            Err(e) => format!("Could not save project: {e}"),
        };
    }

    /// A walk of every folder in the sidebar (respecting .gitignore); None without one.
    pub(super) fn walk_folders(&self) -> Option<ignore::Walk> {
        let (first, rest) = self.sidebar.roots.split_first()?;
        let mut b = ignore::WalkBuilder::new(first);
        for r in rest {
            b.add(r);
        }
        Some(b.require_git(false).build())
    }

    // ---- view modes ----------------------------------------------------------------

    /// Full screen with only the text: no sidebar, tabs, status bar or minimap.
    pub(super) fn toggle_distraction_free(&mut self, ctx: &egui::Context) {
        match self.distraction.take() {
            Some(sidebar) => {
                self.sidebar.visible = sidebar;
                self.fullscreen = false;
            }
            None => {
                self.distraction = Some(self.sidebar.visible);
                self.sidebar.visible = false;
                self.fullscreen = true;
            }
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
    }

    /// The minimap is drawn (the setting, unless distraction-free hides it).
    pub(super) fn minimap_shown(&self) -> bool {
        self.settings.minimap && self.distraction.is_none()
    }

    pub(super) fn about_window(&mut self, ctx: &egui::Context) {
        let mut open = self.about_open;
        let mut show_licenses = false;
        egui::Window::new("About Strata").open(&mut open).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            let icon = ctx.data_mut(|d| d.get_temp::<egui::TextureHandle>(egui::Id::new("about-icon"))).unwrap_or_else(|| {
                let data = eframe::icon_data::from_png_bytes(include_bytes!("../../assets/icon-app.png")).expect("bundled icon");
                let image = egui::ColorImage::from_rgba_unmultiplied([data.width as usize, data.height as usize], &data.rgba);
                let tex = ctx.load_texture("about-icon", image, egui::TextureOptions::LINEAR);
                ctx.data_mut(|d| d.insert_temp(egui::Id::new("about-icon"), tex.clone()));
                tex
            });
            ui.set_max_width(340.0);
            ui.vertical_centered(|ui| {
                ui.add(egui::Image::new(&icon).fit_to_exact_size(egui::vec2(96.0, 96.0)));
                ui.heading("Strata");
                let version = match crate::crash::COMMIT {
                    Some(c) => format!("Version {} ({c})", env!("CARGO_PKG_VERSION")),
                    None => format!("Version {}", env!("CARGO_PKG_VERSION")),
                };
                ui.label(egui::RichText::new(version).weak());
                ui.add_space(8.0);
                ui.label(egui::RichText::new("A fast, open-source code and text editor.").strong());
                ui.add_space(2.0);
                ui.label("Native, written in Rust, and built for the keyboard. Your files stay on your computer: no accounts, no telemetry.");
                ui.add_space(8.0);
                ui.label(egui::RichText::new("MIT License · © 2026 Strata contributors").small().weak());
                ui.add_space(4.0);
                show_licenses = ui.button("Licenses and Attribution…").clicked();
            });
        });
        self.about_open = open;
        if show_licenses {
            self.licenses_open = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;
    use crate::document::Document;

    #[test]
    fn jump_back_and_forward_across_tabs() {
        let mut app = StrataApp::new(None, None);
        app.doc().editor.insert_text("one\ntwo\nthree\n");
        app.push_jump();
        app.goto_line(2);
        app.push_doc(Document::from_buffer(None, Buffer::from_str("other")));
        app.jump(true);
        assert_eq!((app.active, app.doc().editor.primary().head), (0, 14));
        app.jump(true);
        assert_eq!(app.doc().editor.primary().head, 14, "no older history");
        app.jump(false);
        assert_eq!(app.active, 1);
    }

    #[test]
    fn definition_in_another_open_file() {
        let mut app = StrataApp::new(None, None);
        app.doc().editor.insert_text("fn helper() {}\n");
        app.doc().highlighter.set_syntax("Rust");
        app.push_doc(Document::from_buffer(None, Buffer::from_str("fn main() { helper(); }\n")));
        app.doc().highlighter.set_syntax("Rust");
        app.doc().editor.select_ranges(&[14..14], 0);
        app.goto_definition();
        assert_eq!(app.active, 0);
        app.jump(true);
        assert_eq!(app.active, 1, "jump back returns to the call");
    }

    #[test]
    fn clear_items_forgets_recent_projects_too() {
        let mut app = StrataApp::new(None, None);
        app.remember_project(Path::new("/p/a.strata-project"));
        app.remember_file(Path::new("/f.txt"));
        app.recent_folders.push(PathBuf::from("/d"));
        app.run(&egui::Context::default(), crate::commands::Cmd::ClearRecent);
        assert!(app.recent_projects.is_empty(), "{:?}", app.recent_projects);
        assert!(app.recent_files.is_empty() && app.recent_folders.is_empty());
    }

    #[test]
    fn failed_save_with_encoding_leaves_the_tab_clean() {
        // The folder doesn't exist, so the save fails (as a cancelled one would).
        let path = std::env::temp_dir().join(format!("strata-no-such-dir-{}", std::process::id())).join("x.txt");
        let mut app = StrataApp::new(None, None);
        app.docs[0] = Document::from_buffer(Some(path), Buffer::from_str("abc"));
        assert!(!app.doc().is_dirty());
        app.save_with_encoding(crate::encoding::Encoding::UTF16LE);
        assert!(!app.doc().is_dirty(), "the tab must not be left marked modified");
        assert_eq!(app.doc().encoding, crate::encoding::Encoding::UTF8);
    }

    #[test]
    fn distraction_free_hides_the_minimap_without_touching_settings() {
        let ctx = egui::Context::default();
        let mut app = StrataApp::new(None, None);
        assert!(app.settings.minimap);
        app.toggle_distraction_free(&ctx);
        assert!(app.settings.minimap, "a view override, never a setting (it would persist and reach other windows)");
        assert!(!app.minimap_shown());
        app.toggle_distraction_free(&ctx);
        assert!(app.settings.minimap && app.minimap_shown());
    }

    #[test]
    fn recent_lists_dedupe_and_cap() {
        let mut v = Vec::new();
        for i in 0..20 {
            push_recent(&mut v, Path::new(&format!("/f{i}")));
        }
        push_recent(&mut v, Path::new("/f10"));
        assert_eq!(v.len(), RECENT_MAX);
        assert_eq!(v[0], PathBuf::from("/f10"));
        assert_eq!(v.iter().filter(|p| p.as_path() == Path::new("/f10")).count(), 1);
    }
}
