//! Text storage: a rope (O(log n) edits and line lookups on huge files).
//!
//! All positions are char indices into the rope. A "line end" is the index
//! just before the line break, so cursors never sit inside a `\r\n` pair.

use crate::encoding::{self, Encoding};
use ropey::Rope;
use std::io::BufWriter;
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
    /// Decoding replaced malformed bytes with U+FFFD: saving would write
    /// those instead of the original bytes.
    pub lossy: bool,
}

impl Buffer {
    pub fn from_str(text: &str) -> Self {
        let rope = Rope::from_str(text);
        let line_ending = detect_line_ending(&rope);
        Self { rope, line_ending, lossy: false }
    }

    /// UTF-8 bytes (hot-exit backups). Invalid UTF-8 is replaced rather
    /// than refused, so any file opens (and `lossy` records that it happened).
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self::decode_as(bytes, Encoding::UTF8)
    }

    /// Bytes read from a file, in whatever encoding they look like
    /// (see [`encoding::decode_detect`]). Any leading BOM is dropped.
    pub fn decode(bytes: &[u8]) -> (Self, Encoding) {
        let (text, enc, lossy) = encoding::decode_detect(bytes);
        (Self { lossy, ..Self::from_str(&text) }, enc)
    }

    /// Bytes in a known encoding. A matching BOM is dropped; malformed
    /// bytes become U+FFFD and set `lossy`.
    pub fn decode_as(bytes: &[u8], enc: Encoding) -> Self {
        let (text, lossy) = encoding::decode(bytes, enc);
        Self { lossy, ..Self::from_str(&text) }
    }

    /// Ok if every character can be saved in `enc`, else a message naming
    /// the first one that can't and where it is.
    pub fn check_encodable(&self, enc: Encoding) -> Result<(), String> {
        let Some((byte, c)) = encoding::find_unencodable(self.rope.chunks(), enc) else { return Ok(()) };
        let idx = self.rope.byte_to_char(byte);
        let line = self.rope.char_to_line(idx);
        let col = idx - self.rope.line_to_char(line);
        Err(format!("'{c}' (U+{:04X}) on line {}, column {} can't be saved as {}", c as u32, line + 1, col + 1, enc.name()))
    }

    /// Save in `enc` without ever leaving a half-written file: write a temp
    /// file next to the target, flush it to disk, then rename it over the
    /// original. Symlinks are followed, so the link stays a link. Text the
    /// encoding can't represent fails with `InvalidData` before anything is
    /// written.
    pub fn write_to(&self, path: &Path, enc: Encoding) -> std::io::Result<()> {
        self.check_encodable(enc).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        write_atomic(path, |out| encoding::encode_to(self.rope.chunks(), enc, out))
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
        String::from(self.rope.slice(self.line_start(line)..self.line_end(line)))
    }

    /// The line's chars without its line break, without allocating.
    pub fn line_chars(&self, line: usize) -> impl Iterator<Item = char> + '_ {
        self.rope.slice(self.line_start(line)..self.line_end(line)).chars()
    }

    /// Every line (break included) as string chunks, without allocating.
    pub fn line_chunks(&self) -> impl Iterator<Item = impl Iterator<Item = &str>> {
        self.rope.lines().map(|l| l.chunks())
    }

    /// Lines from `line` on, each as string chunks (break included).
    pub fn lines_from(&self, line: usize) -> impl Iterator<Item = impl Iterator<Item = &str>> {
        self.rope.lines_at(line.min(self.len_lines())).map(|l| l.chunks())
    }

    /// Lines before `line`, nearest first, each as string chunks (break included).
    pub fn lines_before(&self, line: usize) -> impl Iterator<Item = impl Iterator<Item = &str>> {
        self.rope.lines_at(line.min(self.len_lines())).reversed().map(|l| l.chunks())
    }

    /// The text as it is now, for a worker thread (cheap: chunks are shared).
    pub fn snapshot(&self) -> Rope {
        self.rope.clone()
    }

    /// The text in `range`. (Copies chunk by chunk: `to_string` would go
    /// through `fmt` and take several times as long on big ranges.)
    pub fn slice(&self, range: Range<usize>) -> String {
        String::from(self.rope.slice(range))
    }

    /// The chars in `range`, without copying them out.
    pub fn chars_in(&self, range: Range<usize>) -> impl Iterator<Item = char> + '_ {
        self.rope.slice(range).chars()
    }

    /// The text in `range` as string chunks, without copying.
    pub fn chunks_in(&self, range: Range<usize>) -> impl Iterator<Item = &str> {
        self.rope.slice(range).chunks()
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

/// Replace `path` with what `fill` writes, never leaving a half-written
/// file: write a temp file next to the target, flush it to disk, then rename
/// it over the original. If anything goes wrong while writing (disk full, I/O
/// error) the temp file is removed and the original is untouched.
///
/// - Symlinks are followed (the link stays a link).
/// - The temp file is created exclusively (`O_CREAT|O_EXCL`, which also
///   refuses a symlink planted at its name) under a fresh name, with the
///   original's mode from the start; owner and mode are then copied through
///   the open handle (owner best-effort). Extended attributes and ACLs are
///   not copied.
/// - A file with several hard links is updated in place (after the new text
///   is safely on disk in the temp file) so every name sees the new text; if
///   that copy fails, the temp file is kept and named in the error.
/// - Only when the folder doesn't let us create a file at all do we fall
///   back to writing the target in place.
pub fn write_atomic(path: &Path, fill: impl Fn(&mut BufWriter<std::fs::File>) -> std::io::Result<()>) -> std::io::Result<()> {
    use std::io::{Seek, Write};
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let meta = std::fs::metadata(&target).ok();
    let dir = target.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let (tmp, file) = match create_temp(dir, &name, meta.as_ref()) {
        Ok(t) => t,
        // Can't create files in the folder (but maybe can write the file): write in place.
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            let mut out = BufWriter::new(std::fs::File::create(&target)?);
            fill(&mut out)?;
            return out.into_inner().map_err(|e| e.into_error())?.sync_all();
        }
        Err(e) => return Err(e),
    };
    let written = (|| {
        let mut out = BufWriter::new(file);
        fill(&mut out)?;
        out.flush()?;
        let file = out.into_inner().map_err(|e| e.into_error())?;
        if let Some(meta) = &meta {
            copy_owner_and_mode(&file, meta);
        }
        file.sync_all()?;
        Ok(file)
    })();
    let mut file = match written {
        Ok(f) => f,
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
    };
    if !meta.as_ref().is_some_and(hard_linked) {
        return std::fs::rename(&tmp, &target).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        });
    }
    // Other names share this file: renaming would split them off with the old
    // text. Copy the (safely written) temp file into the original instead.
    let copied = (|| {
        file.rewind()?;
        let mut dest = std::fs::OpenOptions::new().write(true).truncate(true).open(&target)?;
        std::io::copy(&mut file, &mut dest)?;
        dest.sync_all()
    })();
    match copied {
        Ok(()) => {
            let _ = std::fs::remove_file(&tmp);
            Ok(())
        }
        Err(e) => Err(std::io::Error::new(e.kind(), format!("{e} (the new text is kept in {})", tmp.display()))),
    }
}

/// A new, exclusively created temp file next to `name` in `dir`, open for
/// reading back too (for the hard-link copy). Retries under another name if
/// one is taken.
fn create_temp(dir: &Path, name: &str, meta: Option<&std::fs::Metadata>) -> std::io::Result<(std::path::PathBuf, std::fs::File)> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
    let mut opts = std::fs::OpenOptions::new();
    // create_new = O_CREAT|O_EXCL: never opens an existing file and never
    // follows a symlink at the final component (so no O_NOFOLLOW needed).
    opts.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        if let Some(m) = meta {
            opts.mode(m.permissions().mode() & 0o777);
        }
    }
    #[cfg(not(unix))]
    let _ = meta;
    let mut last = None;
    for _ in 0..64 {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp = dir.join(format!(".{name}.strata-{}-{nanos:x}-{n}.tmp", std::process::id()));
        match opts.open(&tmp) {
            Ok(f) => return Ok((tmp, f)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => last = Some(e),
            Err(e) => return Err(e),
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("no free temp file name")))
}

/// Give the temp file the original's owner (best-effort: only root can give
/// files away) and permissions, through the handle rather than by path.
fn copy_owner_and_mode(file: &std::fs::File, meta: &std::fs::Metadata) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // Before the chmod: chown clears setuid/setgid bits.
        let _ = std::os::unix::fs::fchown(file, Some(meta.uid()), Some(meta.gid()));
    }
    let _ = file.set_permissions(meta.permissions());
}

#[cfg(unix)]
fn hard_linked(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    meta.is_file() && meta.nlink() > 1
}

/// (Windows: the link count needs an unstable API; a rename replaces just this name.)
#[cfg(not(unix))]
fn hard_linked(_: &std::fs::Metadata) -> bool {
    false
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
            Buffer::from_str("new").write_to(&dir.join("link.sh"), Encoding::UTF8).unwrap();
            assert!(std::fs::symlink_metadata(dir.join("link.sh")).unwrap().file_type().is_symlink());
            assert_eq!(std::fs::metadata(&real).unwrap().permissions().mode() & 0o777, 0o755);
        }
        #[cfg(not(unix))]
        Buffer::from_str("new").write_to(&real, Encoding::UTF8).unwrap();
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "new");
        let leftovers = std::fs::read_dir(&dir).unwrap().filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().ends_with(".tmp")).count();
        assert_eq!(leftovers, 0);
        std::fs::remove_dir_all(dir).ok();
    }

    fn scratch_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("strata-buf-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn leftover_temps(dir: &Path) -> usize {
        std::fs::read_dir(dir).unwrap().filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().ends_with(".tmp")).count()
    }

    #[test]
    fn a_failed_write_leaves_the_original_untouched() {
        // Disk full / I/O error halfway through: the save fails and the
        // original keeps every byte (it used to be rewritten in place).
        let dir = scratch_dir("enospc");
        let path = dir.join("precious.txt");
        std::fs::write(&path, "original contents").unwrap();
        let err = write_atomic(&path, |out| {
            use std::io::Write;
            out.write_all(b"partial")?;
            out.flush()?;
            Err(std::io::Error::other("No space left on device"))
        })
        .unwrap_err();
        assert!(err.to_string().contains("No space"), "{err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "original contents");
        assert_eq!(leftover_temps(&dir), 0);
        std::fs::remove_dir_all(dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn hard_links_all_see_the_saved_text() {
        let dir = scratch_dir("hardlink");
        let a = dir.join("a.txt");
        std::fs::write(&a, "old").unwrap();
        std::fs::hard_link(&a, dir.join("b.txt")).unwrap();
        Buffer::from_str("new").write_to(&a, Encoding::UTF8).unwrap();
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(dir.join("b.txt")).unwrap(), "new", "the other name still points at the same file");
        assert_eq!(leftover_temps(&dir), 0);
        std::fs::remove_dir_all(dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn read_only_files_keep_their_mode_and_a_locked_folder_saves_in_place() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch_dir("modes");
        let ro = dir.join("ro.txt");
        std::fs::write(&ro, "old").unwrap();
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o444)).unwrap();
        Buffer::from_str("new").write_to(&ro, Encoding::UTF8).unwrap();
        assert_eq!(std::fs::metadata(&ro).unwrap().permissions().mode() & 0o777, 0o444);
        assert_eq!(std::fs::read_to_string(&ro).unwrap(), "new");

        // A folder we can't create files in, holding a file we can write.
        let locked = dir.join("locked");
        std::fs::create_dir(&locked).unwrap();
        let f = locked.join("f.txt");
        std::fs::write(&f, "old").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
        let r = Buffer::from_str("new").write_to(&f, Encoding::UTF8);
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        r.unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "new");
        assert_eq!(leftover_temps(&locked), 0);
        std::fs::remove_dir_all(dir).ok();
    }

    fn text(b: &Buffer) -> String {
        b.slice(0..b.len_chars())
    }

    /// Save through `write_to` and return the bytes on disk.
    fn saved(b: &Buffer, enc: Encoding) -> Vec<u8> {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("strata-enc-{}-{n}.txt", std::process::id()));
        b.write_to(&path, enc).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(path).ok();
        bytes
    }

    #[test]
    fn boms_are_detected_stripped_and_written_back() {
        let id = Encoding::from_id;
        for (enc, bytes) in [
            (Encoding::UTF8_BOM, b"\xEF\xBB\xBFh\xC3\xA9\n".to_vec()),
            (Encoding::UTF16LE_BOM, b"\xFF\xFEh\x00\xE9\x00\n\x00".to_vec()),
            (Encoding::UTF16BE_BOM, b"\xFE\xFF\x00h\x00\xE9\x00\n".to_vec()),
        ] {
            let (b, found) = Buffer::decode(&bytes);
            assert_eq!(found, enc);
            assert_eq!(text(&b), "hé\n", "{}", enc.name());
            assert!(!b.lossy);
            assert_eq!(saved(&b, enc), bytes, "{}", enc.name());
            // Reopened explicitly, the BOM is still not part of the text.
            assert_eq!(text(&Buffer::decode_as(&bytes, enc)), "hé\n");
        }
        assert_eq!(id("utf-8-bom"), Some(Encoding::UTF8_BOM));
        assert_eq!(text(&Buffer::decode_as(b"\xEF\xBB\xBFx", Encoding::UTF8)), "x");
    }

    #[test]
    fn bomless_utf16_is_detected() {
        let le: Vec<u8> = "plain text\r\n".encode_utf16().flat_map(u16::to_le_bytes).collect();
        let be: Vec<u8> = "plain text\r\n".encode_utf16().flat_map(u16::to_be_bytes).collect();
        let (b, enc) = Buffer::decode(&le);
        assert_eq!((text(&b).as_str(), enc), ("plain text\r\n", Encoding::UTF16LE));
        assert_eq!(saved(&b, enc), le, "no BOM added on save");
        let (b, enc) = Buffer::decode(&be);
        assert_eq!((text(&b).as_str(), enc), ("plain text\r\n", Encoding::UTF16BE));
        assert_eq!(b.line_ending(), LineEnding::CrLf);
    }

    #[test]
    fn invalid_utf8_falls_back_to_windows_1252_losslessly() {
        let bytes = b"caf\xE9 \x80 \x81\n";
        let (b, enc) = Buffer::decode(bytes);
        assert_eq!(enc, Encoding::WINDOWS_1252);
        assert_eq!(text(&b), "café € \u{81}\n");
        assert!(!b.lossy);
        assert_eq!(saved(&b, enc), bytes);
        let (b, enc) = Buffer::decode("café".as_bytes());
        assert_eq!((text(&b).as_str(), enc), ("café", Encoding::UTF8));
    }

    #[test]
    fn cjk_encodings_round_trip() {
        for (id, s) in [("shift_jis", "日本語のテキスト\n"), ("gbk", "中文文本\n"), ("big5", "繁體中文\n"), ("euc-kr", "한국어\n"), ("euc-jp", "日本語\n")] {
            let enc = Encoding::from_id(id).unwrap();
            let bytes = saved(&Buffer::from_str(s), enc);
            assert_ne!(bytes, s.as_bytes(), "{id}");
            let b = Buffer::decode_as(&bytes, enc);
            assert_eq!(text(&b), s, "{id}");
            assert!(!b.lossy);
        }
        // Shift_JIS for "日本" is 0x93FA 0x967B.
        assert_eq!(saved(&Buffer::from_str("日本"), Encoding::from_id("shift_jis").unwrap()), b"\x93\xFA\x96\x7B");
    }

    #[test]
    fn unencodable_text_is_reported_and_not_saved() {
        let b = Buffer::from_str("ok\nsome 漢 here");
        let err = b.check_encodable(Encoding::WINDOWS_1252).unwrap_err();
        assert!(err.contains('漢') && err.contains("line 2, column 6"), "{err}");
        assert!(b.check_encodable(Encoding::UTF16LE).is_ok());
        assert!(b.check_encodable(Encoding::from_id("gbk").unwrap()).is_ok());

        let path = std::env::temp_dir().join(format!("strata-enc-bad-{}.txt", std::process::id()));
        std::fs::write(&path, "original").unwrap();
        let e = b.write_to(&path, Encoding::WINDOWS_1252).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "original", "file untouched");
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn malformed_bytes_in_a_chosen_encoding_are_lossy() {
        let b = Buffer::decode_as(b"\x82", Encoding::from_id("shift_jis").unwrap());
        assert!(b.lossy);
        assert!(!Buffer::decode_as(b"\x82\xA0", Encoding::from_id("shift_jis").unwrap()).lossy);
    }
}
