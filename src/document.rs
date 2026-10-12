//! One open tab: a buffer with its own cursors, undo, scroll and highlighting.

use crate::buffer::{Buffer, LineEnding};
use crate::editor::Editor;
use crate::find::Finder;
use crate::highlight::Highlighter;
use crate::view::EditorView;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub struct Document {
    pub path: Option<PathBuf>,
    pub editor: Editor,
    pub highlighter: Highlighter,
    pub view: EditorView,
    pub finder: Finder,
    /// Opened by a single click in the sidebar: replaced by the next preview
    /// unless edited or opened for real.
    pub preview: bool,
    /// Restored from a hot-exit backup: unsaved even though undo history is empty.
    pub restored_dirty: bool,
    /// Modification time when last read or written, to notice outside changes.
    pub disk_mtime: Option<SystemTime>,
    /// The file changed on disk while this tab had unsaved edits.
    pub conflict: bool,
    /// The file was deleted or moved away outside Strata.
    pub deleted: bool,
}

pub fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

impl Document {
    pub fn untitled() -> Self {
        Self::from_buffer(None, Buffer::from_str(""))
    }

    pub fn from_buffer(path: Option<PathBuf>, buffer: Buffer) -> Self {
        let mut highlighter = Highlighter::new();
        highlighter.detect(path.as_deref(), &buffer.line_text(0));
        Self {
            disk_mtime: path.as_deref().and_then(mtime),
            path,
            editor: Editor::new(buffer),
            highlighter,
            view: EditorView::new(),
            finder: Finder::default(),
            preview: false,
            restored_dirty: false,
            conflict: false,
            deleted: false,
        }
    }

    /// A path that doesn't exist yet opens as a new, empty file.
    pub fn open(path: PathBuf) -> std::io::Result<Self> {
        let buffer = if path.exists() { Buffer::from_bytes(&std::fs::read(&path)?) } else { Buffer::from_str("") };
        Ok(Self::from_buffer(Some(path), buffer))
    }

    pub fn is_dirty(&self) -> bool {
        self.restored_dirty || self.editor.is_dirty()
    }

    /// Untitled, unchanged and empty: safe to replace without asking.
    pub fn is_blank(&self) -> bool {
        self.path.is_none() && !self.is_dirty() && self.editor.buffer.len_chars() == 0
    }

    pub fn name(&self) -> String {
        self.path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.untitled_name())
    }

    /// Untitled tabs are named after their first line, like Sublime.
    fn untitled_name(&self) -> String {
        let first = self.editor.buffer.line_text(0);
        let first = first.trim();
        if first.is_empty() {
            return "untitled".into();
        }
        let mut name: String = first.chars().take(28).collect();
        if first.chars().count() > 28 {
            name.push('…');
        }
        name
    }

    pub fn write_to(&mut self, path: PathBuf) -> std::io::Result<()> {
        self.editor.buffer.write_to(&path)?;
        self.editor.mark_saved();
        self.restored_dirty = false;
        self.conflict = false;
        self.deleted = false;
        self.disk_mtime = mtime(&path);
        if self.path.as_ref() != Some(&path) {
            self.highlighter.detect(Some(&path), &self.editor.buffer.line_text(0));
        }
        self.path = Some(path);
        self.preview = false;
        Ok(())
    }

    /// Re-read the file if it changed on disk. Unsaved edits are never
    /// overwritten: the tab is flagged instead. Returns true if reloaded.
    pub fn check_disk(&mut self) -> bool {
        let Some(path) = &self.path else { return false };
        let now = mtime(path);
        if now.is_none() {
            // Gone from disk: keep the text as unsaved so hot exit backs it up.
            if self.disk_mtime.is_some() && !path.exists() {
                self.deleted = true;
                self.restored_dirty = true;
                self.disk_mtime = None;
            }
            return false;
        }
        if now == self.disk_mtime {
            return false;
        }
        if self.is_dirty() {
            self.conflict = true;
            self.disk_mtime = now;
            return false;
        }
        let Ok(bytes) = std::fs::read(path) else { return false };
        self.reload(Buffer::from_bytes(&bytes));
        self.disk_mtime = now;
        true
    }

    /// Convert every line break (one undo step); the cursor keeps its line and column.
    pub fn set_line_ending(&mut self, ending: LineEnding) {
        let (line, col) = self.editor.cursor_line_col();
        let text = self.editor.text_snapshot();
        let lf = text.replace("\r\n", "\n");
        let new = if ending == LineEnding::CrLf { lf.replace('\n', "\r\n") } else { lf };
        if new != *text {
            let len = self.editor.buffer.len_chars();
            self.editor.replace_ranges(vec![(0..len, new)]);
        }
        self.editor.buffer.set_line_ending(ending);
        let b = &self.editor.buffer;
        let at = (b.line_start(line - 1) + col - 1).min(b.line_end(line - 1));
        self.editor.select_ranges(&[at..at], 0);
    }

    /// True if saving would overwrite changes made outside Strata.
    pub fn changed_on_disk(&self) -> bool {
        self.conflict || self.path.as_deref().and_then(mtime).is_some_and(|now| Some(now) != self.disk_mtime)
    }

    /// Swap in new text, keeping the cursor on (about) the same spot.
    pub fn reload(&mut self, buffer: Buffer) {
        let head = self.editor.primary().head.min(buffer.len_chars());
        let (tab_width, use_spaces) = (self.editor.tab_width, self.editor.use_spaces);
        self.editor = Editor::new(buffer);
        self.editor.tab_width = tab_width;
        self.editor.use_spaces = use_spaces;
        self.editor.select_ranges(&[head..head], 0);
        self.highlighter.invalidate_from(0);
        self.finder = Finder::default();
        self.conflict = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reloads_clean_files_and_flags_dirty_ones() {
        let dir = std::env::temp_dir().join(format!("strata-doc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.txt");
        std::fs::write(&path, "one\n").unwrap();
        let mut doc = Document::open(path.clone()).unwrap();
        doc.editor.select_ranges(&[2..2], 0);

        // Force a different mtime than the one recorded at open.
        doc.disk_mtime = Some(SystemTime::UNIX_EPOCH);
        std::fs::write(&path, "one two\n").unwrap();
        assert!(doc.check_disk());
        assert_eq!(doc.editor.buffer.line_text(0), "one two");
        assert_eq!(doc.editor.primary().head, 2);

        doc.editor.insert_text("X");
        doc.disk_mtime = Some(SystemTime::UNIX_EPOCH);
        assert!(!doc.check_disk(), "unsaved edits are kept");
        assert!(doc.conflict);
        assert!(doc.changed_on_disk());

        std::fs::remove_file(&path).unwrap();
        doc.conflict = false;
        doc.disk_mtime = Some(SystemTime::UNIX_EPOCH);
        doc.check_disk();
        assert!(doc.deleted && doc.is_dirty(), "a deleted file's text is kept as unsaved");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn converting_line_endings_is_one_undo_step() {
        let mut doc = Document::from_buffer(None, Buffer::from_str("a\nbc\n"));
        doc.editor.select_ranges(&[3..3], 0);
        doc.set_line_ending(LineEnding::CrLf);
        assert_eq!(doc.editor.buffer.slice(0..doc.editor.buffer.len_chars()), "a\r\nbc\r\n");
        assert_eq!(doc.editor.cursor_line_col(), (2, 2));
        assert!(doc.is_dirty());
        doc.editor.undo();
        assert_eq!(doc.editor.buffer.slice(0..doc.editor.buffer.len_chars()), "a\nbc\n");
    }

    #[test]
    fn untitled_tabs_take_their_first_line_as_name() {
        let mut doc = Document::untitled();
        assert_eq!(doc.name(), "untitled");
        assert!(doc.is_blank());
        doc.editor.insert_text("  shopping list\nmilk");
        assert_eq!(doc.name(), "shopping list");
        assert!(!doc.is_blank());
    }
}
