//! Text storage: a rope (O(log n) edits and line lookups on huge files).
//!
//! All positions are char indices into the rope. A "line end" is the index
//! just before the line break, so cursors never sit inside a `\r\n` pair.

use ropey::Rope;
use std::io::{BufWriter, Write};
use std::ops::Range;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    CrLf,
}

impl LineEnding {
    pub fn as_str(self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::CrLf => "\r\n",
        }
    }
}

pub struct Buffer {
    rope: Rope,
    line_ending: LineEnding,
    /// The file wasn't valid UTF-8: saving would replace the bad bytes.
    pub lossy: bool,
}

impl Buffer {
    pub fn from_str(text: &str) -> Self {
        let rope = Rope::from_str(text);
        let line_ending = detect_line_ending(&rope);
        Self { rope, line_ending, lossy: false }
    }

    /// Invalid UTF-8 is replaced rather than refused, so any file opens
    /// (and `lossy` records that it happened).
    pub fn from_bytes(bytes: &[u8]) -> Self {
        let text = String::from_utf8_lossy(bytes);
        let lossy = matches!(text, std::borrow::Cow::Owned(_));
        Self { lossy, ..Self::from_str(&text) }
    }

    /// Save without ever leaving a half-written file: write a temp file next
    /// to the target, flush it to disk, then rename it over the original.
    /// Symlinks are followed, so the link stays a link.
    pub fn write_to(&self, path: &Path) -> std::io::Result<()> {
        let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
        let dir = target.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
        let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let tmp = dir.join(format!(".{name}.strata-{}.tmp", std::process::id()));
        match self.write_file(&tmp) {
            Ok(()) => {
                if let Ok(meta) = std::fs::metadata(&target) {
                    let _ = std::fs::set_permissions(&tmp, meta.permissions());
                }
                std::fs::rename(&tmp, &target).inspect_err(|_| {
                    let _ = std::fs::remove_file(&tmp);
                })
            }
            // Can't create files in the folder (but maybe can write the file): write in place.
            Err(_) => {
                let _ = std::fs::remove_file(&tmp);
                self.write_file(&target)
            }
        }
    }

    fn write_file(&self, path: &Path) -> std::io::Result<()> {
        let mut out = BufWriter::new(std::fs::File::create(path)?);
        for chunk in self.rope.chunks() {
            out.write_all(chunk.as_bytes())?;
        }
        out.into_inner().map_err(|e| e.into_error())?.sync_all()
    }

    pub fn line_ending(&self) -> LineEnding {
        self.line_ending
    }

    /// The ending used for new lines (Enter, paste). Existing text is unchanged.
    pub fn set_line_ending(&mut self, ending: LineEnding) {
        self.line_ending = ending;
    }

    pub fn len_chars(&self) -> usize {
        self.rope.len_chars()
    }

    pub fn len_lines(&self) -> usize {
        self.rope.len_lines()
    }

    pub fn char(&self, idx: usize) -> char {
        self.rope.char(idx)
    }

    pub fn char_to_byte(&self, idx: usize) -> usize {
        self.rope.char_to_byte(idx)
    }

    pub fn byte_to_char(&self, idx: usize) -> usize {
        self.rope.byte_to_char(idx)
    }

    pub fn line_of(&self, idx: usize) -> usize {
        self.rope.char_to_line(idx.min(self.len_chars()))
    }

    pub fn line_start(&self, line: usize) -> usize {
        self.rope.line_to_char(line.min(self.len_lines()))
    }

    /// Index just before the line's break (or end of text on the last line).
    pub fn line_end(&self, line: usize) -> usize {
        let start = self.line_start(line);
        let slice = self.rope.line(line);
        let mut len = slice.len_chars();
        while len > 0 && matches!(slice.char(len - 1), '\n' | '\r') {
            len -= 1;
        }
        start + len
    }

    /// The line's text without its line break.
    pub fn line_text(&self, line: usize) -> String {
        self.rope
            .slice(self.line_start(line)..self.line_end(line))
            .to_string()
    }

    /// The line's chars without its line break, without allocating.
    pub fn line_chars(&self, line: usize) -> impl Iterator<Item = char> + '_ {
        self.rope.slice(self.line_start(line)..self.line_end(line)).chars()
    }

    /// Every line (break included) as string chunks, without allocating.
    pub fn line_chunks(&self) -> impl Iterator<Item = impl Iterator<Item = &str>> {
        self.rope.lines().map(|l| l.chunks())
    }

    pub fn slice(&self, range: Range<usize>) -> String {
        self.rope.slice(range).to_string()
    }

    pub fn insert(&mut self, at: usize, text: &str) {
        self.rope.insert(at, text);
    }

    pub fn remove(&mut self, range: Range<usize>) {
        if !range.is_empty() {
            self.rope.remove(range);
        }
    }
}

fn detect_line_ending(rope: &Rope) -> LineEnding {
    if rope.len_lines() > 1 {
        let first = rope.line(0);
        let n = first.len_chars();
        if n >= 2 && first.char(n - 2) == '\r' && first.char(n - 1) == '\n' {
            return LineEnding::CrLf;
        }
        return LineEnding::Lf;
    }
    // New/one-line files follow the platform convention.
    if cfg!(windows) { LineEnding::CrLf } else { LineEnding::Lf }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_end_excludes_crlf() {
        let b = Buffer::from_str("ab\r\ncd\n");
        assert_eq!(b.line_ending(), LineEnding::CrLf);
        assert_eq!(b.len_lines(), 3);
        assert_eq!(b.line_end(0), 2);
        assert_eq!(b.line_start(1), 4);
        assert_eq!(b.line_text(1), "cd");
        assert_eq!(b.line_text(2), "");
    }

    #[test]
    fn lossy_utf8_opens() {
        let b = Buffer::from_bytes(b"ok\xffok");
        assert_eq!(b.line_text(0), "ok\u{fffd}ok");
        assert!(b.lossy);
        assert!(!Buffer::from_bytes("café".as_bytes()).lossy);
    }

    #[test]
    fn save_replaces_atomically_and_keeps_symlinks_and_mode() {
        let dir = std::env::temp_dir().join(format!("strata-buf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let real = dir.join("real.sh");
        std::fs::write(&real, "old").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o755)).unwrap();
            std::os::unix::fs::symlink(&real, dir.join("link.sh")).unwrap();
            Buffer::from_str("new").write_to(&dir.join("link.sh")).unwrap();
            assert!(std::fs::symlink_metadata(dir.join("link.sh")).unwrap().file_type().is_symlink());
            assert_eq!(std::fs::metadata(&real).unwrap().permissions().mode() & 0o777, 0o755);
        }
        #[cfg(not(unix))]
        Buffer::from_str("new").write_to(&real).unwrap();
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "new");
        let leftovers = std::fs::read_dir(&dir).unwrap().filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().ends_with(".tmp")).count();
        assert_eq!(leftovers, 0);
        std::fs::remove_dir_all(dir).ok();
    }
}
