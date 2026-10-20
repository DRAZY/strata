//! One open tab: a buffer with its own cursors, undo, scroll and highlighting.

use crate::buffer::{Buffer, LineEnding};
use crate::editor::Editor;
use crate::encoding::Encoding;
use crate::find::Finder;
use crate::highlight::Highlighter;
use crate::view::EditorView;
use std::io;
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
    /// Size in bytes then (None: unknown, e.g. restored from a session): a
    /// rewrite within one mtime tick usually changes it.
    pub disk_len: Option<u64>,
    /// The file changed on disk while this tab had unsaved edits.
    pub conflict: bool,
    /// The file was deleted or moved away outside Strata.
    pub deleted: bool,
    /// Split pane (group) the tab lives in.
    pub group: usize,
    /// When the tab was last activated; each pane shows its most recent tab.
    pub stamp: u64,
    /// Lines changed since the last commit (git gutter), for `git_version`.
    pub git_changes: crate::git::Changes,
    pub git_version: Option<u64>,
    /// Word wrap chosen for this tab; None follows the setting and syntax.
    pub wrap: Option<bool>,
    /// How the file's bytes are read and written: detected on open, changed
    /// by "Reopen with Encoding" / "Save with Encoding".
    pub encoding: Encoding,
    /// Tab name for a generated, pathless tab (e.g. "Default Key Bindings").
    pub title: Option<String>,
    /// Tabs showing one buffer (New View into File, Compare) share this id;
    /// see `app::clones`.
    pub view_group: Option<u64>,
    /// A side-by-side Compare tab: `editor` is its right (editable) side.
    pub compare: Option<Box<crate::compare::Compare>>,
    /// Status bar word count and the text/selection state it was counted for.
    pub word_count: Option<(u64, Option<String>)>,
    /// Status bar count of hidden and direction-changing characters.
    pub hidden_count: crate::hidden::Counter,
    /// Opened in large-file mode (see `LargeFile`).
    pub large: Option<LargeFile>,
    /// Settings resolved for this tab, and what was set for it alone (see `app::doc_settings`).
    pub local: crate::doc_settings::Local,
    /// The encoding was guessed (the file isn't Unicode) and is worth
    /// pointing out: see `encoding::Detected::worth_mentioning`.
    pub encoding_guessed: bool,
    /// Not text: an image preview or a read-only hex view (see `binary`).
    /// The editor is empty and read-only, the tab is never unsaved, and it
    /// can't be saved.
    pub binary: Option<Box<crate::binary::BinaryView>>,
}

/// Enough of a file's metadata to tell that it was rewritten, even to the
/// same size with its modification time put back: the system's change
/// time (which programs can't set), and on Unix which file it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileId {
    len: u64,
    mtime: Option<SystemTime>,
    change: (i64, i64),
    node: (u64, u64),
}

pub fn file_id(path: &Path) -> Option<FileId> {
    let m = std::fs::metadata(path).ok()?;
    #[cfg(unix)]
    let (change, node) = {
        use std::os::unix::fs::MetadataExt;
        ((m.ctime(), m.ctime_nsec()), (m.dev(), m.ino()))
    };
    #[cfg(windows)]
    let (change, node) = (windows_change_time(path)?, (0, 0));
    #[cfg(not(any(unix, windows)))]
    let (change, node) = ((0, 0), (0, 0));
    Some(FileId { len: m.len(), mtime: m.modified().ok(), change, node })
}

/// The file's ChangeTime (any write or metadata change updates it).
#[cfg(windows)]
fn windows_change_time(path: &Path) -> Option<(i64, i64)> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{FILE_BASIC_INFO, FileBasicInfo, GetFileInformationByHandleEx};
    let file = std::fs::File::open(path).ok()?;
    // SAFETY: `info` is a plain struct of the size passed, filled by the call.
    unsafe {
        let mut info: FILE_BASIC_INFO = std::mem::zeroed();
        let ok = GetFileInformationByHandleEx(file.as_raw_handle() as _, FileBasicInfo, &mut info as *mut _ as *mut core::ffi::c_void, std::mem::size_of::<FILE_BASIC_INFO>() as u32);
        (ok != 0).then_some((info.ChangeTime, info.LastWriteTime))
    }
}

/// SHA-256 of a file's bytes.
pub fn content_hash(bytes: &[u8]) -> [u8; 32] {
    use sha2::Digest;
    sha2::Sha256::digest(bytes).into()
}

/// A text file read and decoded, not yet a tab (see `Document::load_text`).
pub struct Loaded {
    pub path: PathBuf,
    pub buffer: Buffer,
    encoding: Encoding,
    guessed: bool,
    /// Large-file mode: (because of a long line, the longest line's bytes).
    large: Option<(bool, usize)>,
    /// Modification time and size before reading.
    pub disk: (Option<SystemTime>, Option<u64>),
    /// SHA-256 of the bytes it was decoded from.
    pub hash: [u8; 32],
}

/// Images are shrunk to fit this many pixels a side (what GPUs take).
pub const MAX_IMAGE_SIDE: u32 = 8192;

/// A file opened in large-file mode, like Sublime's and VS Code's: it shows
/// as Plain Text, and the minimap, word wrap, bracket colors and sticky
/// scroll are off for it. Choosing a syntax in the status bar still
/// highlights it (in the background, a screenful at a time).
#[derive(Clone, Debug, PartialEq)]
pub struct LargeFile {
    /// The syntax it would have had.
    pub syntax: String,
    /// Why: a line longer than `LONG_LINE_BYTES` (rather than the size).
    pub long_lines: bool,
    /// Bytes in its longest line when opened.
    pub longest_line: usize,
}

/// Word wrap stays off for files with a line longer than this (bytes): each
/// edit would re-wrap the whole line.
pub const MAX_WRAP_LINE: usize = 200_000;

/// Files bigger than this open in large-file mode unless told otherwise
/// (the `large_file_mb` setting).
pub const LARGE_FILE_BYTES: u64 = 20 << 20;
/// A line longer than this (minified code, one-line JSON) also means large-file mode.
pub const LONG_LINE_BYTES: usize = 10_000;

/// Length in bytes of the longest line (fast: a byte scan).
fn longest_line(bytes: &[u8]) -> usize {
    bytes.split(|&b| b == b'\n').map(<[u8]>::len).max().unwrap_or(0)
}

pub fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Modification time and size of the file at `path`.
pub fn stamp(path: &Path) -> (Option<SystemTime>, Option<u64>) {
    match std::fs::metadata(path) {
        Ok(m) => (m.modified().ok(), Some(m.len())),
        Err(_) => (None, None),
    }
}

impl Document {
    pub fn untitled() -> Self {
        Self::from_buffer(None, Buffer::from_str(""))
    }

    pub fn from_buffer(path: Option<PathBuf>, buffer: Buffer) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let mut view = EditorView::new();
        view.id = egui::Id::new(("strata-editor", NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        let mut highlighter = Highlighter::for_editor();
        highlighter.detect(path.as_deref(), &buffer.line_text(0));
        Self {
            disk_mtime: path.as_deref().and_then(mtime),
            disk_len: path.as_deref().and_then(|p| stamp(p).1),
            path,
            editor: Editor::new(buffer),
            highlighter,
            view,
            finder: Finder::default(),
            preview: false,
            restored_dirty: false,
            conflict: false,
            deleted: false,
            group: 0,
            stamp: 0,
            wrap: None,
            encoding: Encoding::UTF8,
            git_changes: Vec::new(),
            git_version: None,
            title: None,
            word_count: None,
            hidden_count: Default::default(),
            large: None,
            local: Default::default(),
            encoding_guessed: false,
            binary: None,
            view_group: None,
            compare: None,
        }
    }

    /// A path that doesn't exist yet opens as a new, empty UTF-8 file.
    /// Otherwise the encoding is detected (see `Buffer::decode`).
    #[cfg(test)]
    pub fn open(path: PathBuf) -> std::io::Result<Self> {
        Self::open_with(path, LARGE_FILE_BYTES)
    }

    /// `open`, in large-file mode if the file has more than `large_bytes`
    /// bytes or a line longer than `LONG_LINE_BYTES`.
    /// A file that isn't text opens in an image preview or the hex view.
    pub fn open_with(path: PathBuf, large_bytes: u64) -> std::io::Result<Self> {
        if path.is_file()
            && let Some(view) = crate::binary::BinaryView::open(&path, MAX_IMAGE_SIDE)?
        {
            let mut doc = Self::from_buffer(Some(path), Buffer::from_str(""));
            doc.show_binary(view);
            return Ok(doc);
        }
        Ok(Self::from_loaded(Self::read_text(path, large_bytes)?))
    }

    /// Read and decode a file as text (no check that it is text: see
    /// `load_text`). Nothing here touches the UI, so it can run on a worker.
    fn read_text(path: PathBuf, large_bytes: u64) -> std::io::Result<Loaded> {
        let disk = stamp(&path);
        let bytes = if path.exists() { std::fs::read(&path)? } else { Vec::new() };
        let longest = longest_line(&bytes);
        let long_lines = longest > LONG_LINE_BYTES;
        let large = (long_lines || bytes.len() as u64 > large_bytes).then_some((long_lines, longest));
        let detected = crate::encoding::detect(&bytes);
        let mut buffer = Buffer::from_str(&detected.text);
        buffer.lossy = detected.lossy;
        let hash = content_hash(&bytes);
        Ok(Loaded { path, buffer, encoding: detected.encoding, guessed: detected.worth_mentioning(), large, disk, hash })
    }

    /// Read and decode a text file off the UI thread, for `from_loaded`;
    /// None if it isn't text (it would open in the image or hex view).
    pub fn load_text(path: PathBuf, large_bytes: u64) -> std::io::Result<Option<Loaded>> {
        if path.is_file() && crate::binary::BinaryView::open(&path, MAX_IMAGE_SIDE)?.is_some() {
            return Ok(None);
        }
        Self::read_text(path, large_bytes).map(Some)
    }

    /// The tab for a file read by `load_text` (or `open_with`).
    pub fn from_loaded(l: Loaded) -> Self {
        let mut doc = Self::from_buffer(Some(l.path), l.buffer);
        doc.encoding = l.encoding;
        doc.encoding_guessed = l.guessed;
        // As of the read: a change since is noticed like any outside change.
        (doc.disk_mtime, doc.disk_len) = l.disk;
        if let Some((long_lines, longest)) = l.large {
            doc.large = Some(LargeFile { syntax: doc.highlighter.syntax_name().to_owned(), long_lines, longest_line: longest });
            doc.highlighter.set_syntax("Plain Text");
        }
        doc
    }

    /// Re-read the file decoding it as `enc` ("Reopen with Encoding").
    /// Refuses (`ErrorKind::Other`) if the tab has unsaved changes — confirm
    /// and discard them first — or has no file. The cursor keeps its line
    /// and column where possible.
    /// A binary tab becomes a text tab: asking for an encoding says it's text.
    pub fn reopen_with_encoding(&mut self, enc: Encoding) -> io::Result<()> {
        if self.is_dirty() {
            return Err(io::Error::other("the tab has unsaved changes"));
        }
        if self.binary.take().is_some() {
            self.editor.read_only = false;
            self.reload_as(enc)?;
            self.highlighter.detect(self.path.as_deref(), &self.editor.buffer.line_text(0));
            return Ok(());
        }
        self.reload_as(enc)
    }

    /// Discard the tab's text and load the file from disk in the current
    /// encoding (the "Reload" action for a conflicted tab).
    pub fn reload_from_disk(&mut self) -> io::Result<()> {
        if self.binary.is_some() {
            let path = self.path.clone().ok_or_else(|| io::Error::other("the tab has no file"))?;
            let (now, len) = stamp(&path);
            if let Some(view) = crate::binary::BinaryView::open(&path, MAX_IMAGE_SIDE)? {
                self.show_binary(view);
                (self.disk_mtime, self.disk_len) = (now, len);
                self.deleted = false;
                return Ok(());
            }
            // It's text now.
            self.binary = None;
            self.editor.read_only = false;
        }
        self.reload_as(self.encoding)
    }

    /// Show `view` instead of text (read-only, plain).
    fn show_binary(&mut self, view: crate::binary::BinaryView) {
        self.binary = Some(Box::new(view));
        self.editor = Editor::new(Buffer::from_str(""));
        self.editor.read_only = true;
        self.highlighter.set_syntax("Plain Text");
        self.restored_dirty = false;
    }

    fn reload_as(&mut self, enc: Encoding) -> io::Result<()> {
        let Some(path) = &self.path else { return Err(io::Error::other("the tab has no file")) };
        let (now, len) = stamp(path);
        let bytes = std::fs::read(path)?;
        let (line, col) = self.editor.cursor_line_col();
        self.reload(Buffer::decode_as(&bytes, enc));
        let b = &self.editor.buffer;
        let line = (line - 1).min(b.len_lines() - 1);
        let at = (b.line_start(line) + col - 1).min(b.line_end(line));
        self.editor.select_ranges(&[at..at], 0);
        self.encoding = enc;
        self.encoding_guessed = false;
        (self.disk_mtime, self.disk_len) = (now, len);
        self.restored_dirty = false;
        self.deleted = false;
        Ok(())
    }

    /// "Save with Encoding": the next save writes `enc`. A file-backed tab
    /// becomes unsaved so the change isn't lost silently; nothing is checked
    /// here — call `editor.buffer.check_encodable(enc)` to warn first.
    pub fn set_encoding(&mut self, enc: Encoding) {
        if enc != self.encoding && self.binary.is_none() {
            self.encoding = enc;
            if self.path.is_some() {
                self.restored_dirty = true;
            }
        }
    }

    /// False for a file with a line too long to wrap (see `MAX_WRAP_LINE`).
    pub fn can_wrap(&self) -> bool {
        self.large.as_ref().is_none_or(|l| l.longest_line <= MAX_WRAP_LINE)
    }

    pub fn is_dirty(&self) -> bool {
        self.binary.is_none() && (self.restored_dirty || self.editor.is_dirty())
    }

    /// Untitled, unchanged and empty: safe to replace without asking.
    pub fn is_blank(&self) -> bool {
        self.path.is_none() && !self.is_dirty() && self.editor.buffer.len_chars() == 0
    }

    pub fn name(&self) -> String {
        if let Some(c) = &self.compare {
            return c.title();
        }
        self.path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .or_else(|| self.title.clone())
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
        self.editor.buffer.write_to(&path, self.encoding)?;
        self.editor.mark_saved();
        self.restored_dirty = false;
        self.conflict = false;
        self.deleted = false;
        (self.disk_mtime, self.disk_len) = stamp(&path);
        if self.path.as_ref() != Some(&path) {
            self.highlighter.detect(Some(&path), &self.editor.buffer.line_text(0));
            // Still large: note the new name's syntax, keep showing it plain.
            if let Some(large) = &mut self.large {
                large.syntax = self.highlighter.syntax_name().to_owned();
                self.highlighter.set_syntax("Plain Text");
            }
        }
        self.path = Some(path);
        self.preview = false;
        Ok(())
    }

    /// Re-read the file if it changed on disk. Unsaved edits are never
    /// overwritten: the tab is flagged instead. Returns true if reloaded.
    pub fn check_disk(&mut self) -> bool {
        let Some(path) = &self.path else { return false };
        let (now, len) = stamp(path);
        if now.is_none() {
            // Gone from disk: keep the text as unsaved so hot exit backs it up.
            if self.disk_mtime.is_some() && !path.exists() {
                self.deleted = true;
                self.restored_dirty = true;
                (self.disk_mtime, self.disk_len) = (None, None);
            }
            return false;
        }
        if !self.differs_from(now, len) {
            return false;
        }
        if self.binary.is_some() {
            return self.reload_from_disk().is_ok();
        }
        if self.is_dirty() {
            self.conflict = true;
            (self.disk_mtime, self.disk_len) = (now, len);
            return false;
        }
        let Ok(bytes) = std::fs::read(path) else { return false };
        self.reload(Buffer::decode_as(&bytes, self.encoding));
        (self.disk_mtime, self.disk_len) = (now, len);
        true
    }

    /// Convert every line break (one undo step); the cursor keeps its line
    /// and column. A choice for this tab, which `.editorconfig` then leaves alone.
    pub fn set_line_ending(&mut self, ending: LineEnding) {
        self.local.chose_line_ending = true;
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
        self.conflict
            || self.path.as_deref().is_some_and(|p| {
                let (now, len) = stamp(p);
                now.is_some() && self.differs_from(now, len)
            })
    }

    /// A file stamped `now` / `len` isn't the one the tab last matched: the
    /// time differs, or the size does (a rewrite within one mtime tick).
    fn differs_from(&self, now: Option<SystemTime>, len: Option<u64>) -> bool {
        now != self.disk_mtime || (self.disk_len.is_some() && len != self.disk_len)
    }

    /// Swap in new text, keeping the cursor on (about) the same spot. The
    /// swap is one undo step (marked saved), so text a reload threw away is
    /// an Undo away; read-only tabs and huge texts start a fresh history.
    pub fn reload(&mut self, buffer: Buffer) {
        /// Above this many chars (old + new) the undo step would hold too much.
        const UNDOABLE_CHARS: usize = 32 << 20;
        let head = self.editor.primary().head.min(buffer.len_chars());
        let len = self.editor.buffer.len_chars();
        if self.editor.read_only || len + buffer.len_chars() > UNDOABLE_CHARS {
            let (tab_width, use_spaces, read_only) = (self.editor.tab_width, self.editor.use_spaces, self.editor.read_only);
            self.editor = Editor::new(buffer);
            self.editor.tab_width = tab_width;
            self.editor.use_spaces = use_spaces;
            self.editor.read_only = read_only;
        } else {
            let new = buffer.slice(0..buffer.len_chars());
            if *self.editor.text_snapshot() != new {
                self.editor.replace_ranges(vec![(0..len, new)]);
            }
            self.editor.buffer.set_line_ending(buffer.line_ending());
            self.editor.buffer.lossy = buffer.lossy;
            self.editor.mark_saved();
        }
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
    fn encodings_are_detected_kept_and_switchable() {
        let dir = std::env::temp_dir().join(format!("strata-doc-enc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("latin.txt");
        // Windows-1252: "café\nnaïve\n"; as Mac Roman 0xE9 is 'È' and 0xEF is 'Ô'.
        std::fs::write(&path, b"caf\xE9\nna\xEFve\n").unwrap();
        let mut doc = Document::open(path.clone()).unwrap();
        assert_eq!(doc.encoding, Encoding::WINDOWS_1252);
        assert_eq!(doc.editor.buffer.line_text(1), "naïve");

        // Outside change: reloaded in the document's encoding, not as UTF-8.
        std::fs::write(&path, b"caf\xE9!\nna\xEFve\n").unwrap();
        doc.disk_mtime = Some(SystemTime::UNIX_EPOCH);
        assert!(doc.check_disk());
        assert_eq!(doc.editor.buffer.line_text(0), "café!");

        // Reopen with Encoding keeps the cursor's line and column.
        doc.editor.select_ranges(&[doc.editor.buffer.line_start(1) + 3..doc.editor.buffer.line_start(1) + 3], 0);
        let mac = Encoding::from_id("macintosh").unwrap();
        doc.reopen_with_encoding(mac).unwrap();
        assert_eq!(doc.encoding, mac);
        assert_eq!(doc.editor.buffer.line_text(0), "cafÈ!");
        assert_eq!(doc.editor.cursor_line_col(), (2, 4));
        assert!(!doc.is_dirty());

        // Dirty tabs are refused.
        doc.editor.insert_text("x");
        assert!(doc.reopen_with_encoding(Encoding::UTF8).is_err());
        assert_eq!(doc.encoding, mac);
        doc.reload_from_disk().unwrap();
        assert!(!doc.is_dirty());
        assert_eq!(doc.editor.buffer.line_text(0), "cafÈ!");

        // Save with Encoding: marks the tab unsaved, then writes the new bytes.
        doc.set_encoding(Encoding::UTF16LE_BOM);
        assert!(doc.is_dirty());
        doc.write_to(path.clone()).unwrap();
        assert!(!doc.is_dirty());
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.starts_with(b"\xFF\xFEc\x00a\x00f\x00\xC8\x00"));
        assert_eq!(Document::open(path.clone()).unwrap().encoding, Encoding::UTF16LE_BOM);
        std::fs::remove_dir_all(dir).ok();
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("strata-doc-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn all(doc: &Document) -> String {
        doc.editor.buffer.slice(0..doc.editor.buffer.len_chars())
    }

    #[test]
    fn a_change_within_the_same_mtime_tick_is_noticed() {
        let dir = scratch("same-tick");
        let path = dir.join("a.txt");
        std::fs::write(&path, "one\n").unwrap();
        let mut doc = Document::open(path.clone()).unwrap();
        let tick = mtime(&path).unwrap();
        // Another program rewrites the file and the clock (or a coarse
        // filesystem) gives it the same modification time.
        std::fs::write(&path, "one two\n").unwrap();
        std::fs::File::options().write(true).open(&path).unwrap().set_modified(tick).unwrap();
        assert_eq!(mtime(&path), Some(tick));
        assert!(doc.changed_on_disk(), "the size differs");
        assert!(doc.check_disk());
        assert_eq!(all(&doc), "one two\n");
        assert!(!doc.changed_on_disk());

        doc.editor.insert_text("mine ");
        std::fs::write(&path, "one two three\n").unwrap();
        std::fs::File::options().write(true).open(&path).unwrap().set_modified(tick).unwrap();
        assert!(!doc.check_disk(), "unsaved edits are kept");
        assert!(doc.conflict);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn reloading_is_undoable() {
        let dir = scratch("undo-reload");
        let path = dir.join("a.txt");
        std::fs::write(&path, "one\n").unwrap();
        let mut doc = Document::open(path.clone()).unwrap();
        doc.editor.select_ranges(&[3..3], 0);
        doc.editor.insert_text(" my edits");
        std::fs::write(&path, "theirs\r\n").unwrap();
        doc.reload_from_disk().unwrap();
        assert_eq!(all(&doc), "theirs\r\n");
        assert_eq!(doc.editor.buffer.line_ending(), LineEnding::CrLf);
        assert!(!doc.is_dirty());
        // The discarded edits are one undo away (and unsaved again).
        doc.editor.undo();
        assert_eq!(all(&doc), "one my edits\n");
        assert!(doc.is_dirty());
        doc.editor.undo();
        assert_eq!(all(&doc), "one\n", "older history is kept too");
        doc.editor.redo();
        doc.editor.redo();
        assert_eq!(all(&doc), "theirs\r\n");
        assert!(!doc.is_dirty());
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
