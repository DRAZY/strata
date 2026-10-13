//! Opening big files: a confirmation above `large_file_warning_mb` or for a
//! file that looks binary, and large-file mode (see `document::LargeFile`)
//! above `large_file_mb`.

use super::StrataApp;
use crate::document::Document;
use std::path::Path;

const MB: f64 = 1024.0 * 1024.0;

/// Why `path` deserves a question before it opens: too big (over
/// `warn_bytes`) or binary (a NUL byte near the start, unless the text is
/// UTF-16). None for files that open without asking, or don't exist yet.
pub fn open_warning(path: &Path, warn_bytes: u64) -> Option<String> {
    let len = std::fs::metadata(path).ok()?.len();
    if len > warn_bytes {
        return Some(format!(
            "{} is {:.0} MB. Opening it reads all of it into memory and may take a while.",
            name(path),
            len as f64 / MB
        ));
    }
    let mut head = vec![0; 8192];
    let n = std::io::Read::read(&mut std::fs::File::open(path).ok()?, &mut head).ok()?;
    head.truncate(n);
    let utf16 = crate::encoding::decode_detect(&head).1.id().starts_with("utf-16");
    (!utf16 && head.contains(&0)).then(|| format!("{} looks like a binary file. Opened as text it shows as gibberish, and saving it could damage it.", name(path)))
}

fn name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

impl StrataApp {
    /// Files past this many bytes open in large-file mode.
    pub(super) fn large_file_bytes(&self) -> u64 {
        (self.settings.large_file_mb.max(0.0) * MB) as u64
    }

    /// Ask before opening a huge or binary file. True to go ahead.
    pub(super) fn confirm_open(&mut self, path: &Path) -> bool {
        let warn = (self.settings.large_file_warning_mb.max(0.0) * MB) as u64;
        let Some(why) = open_warning(path, warn) else { return true };
        let open = "Open Anyway".to_owned();
        let answer = rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Warning)
            .set_title("Open file?")
            .set_description(why)
            .set_buttons(rfd::MessageButtons::OkCancelCustom(open.clone(), "Cancel".into()))
            .show();
        let ok = matches!(answer, rfd::MessageDialogResult::Ok | rfd::MessageDialogResult::Yes) || answer == rfd::MessageDialogResult::Custom(open);
        if !ok {
            self.status = format!("Didn't open {}", path.display());
        }
        ok
    }
}

/// The status line after opening `doc`, saying what large-file mode turned off.
pub fn opened_status(doc: &Document, path: &Path) -> String {
    let Some(large) = &doc.large else { return format!("Opened {}", path.display()) };
    let why = if large.long_lines { "very long lines" } else { "large file" };
    let syntax = if large.syntax == "Plain Text" {
        String::new()
    } else {
        format!(" To highlight it as {0}, choose {0} in the status bar's syntax menu.", large.syntax)
    };
    format!("Opened {} ({why}): syntax highlighting, minimap, word wrap and bracket colors are off.{syntax}", path.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn huge_and_binary_files_ask_first() {
        let dir = std::env::temp_dir().join(format!("strata-large-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let text = dir.join("a.txt");
        std::fs::write(&text, "hello\n".repeat(100)).unwrap();
        assert_eq!(open_warning(&text, 1 << 20), None);
        assert!(open_warning(&text, 100).unwrap().contains("a.txt is 0 MB"));
        let bin = dir.join("a.bin");
        std::fs::write(&bin, b"\x7fELF\x02\x01\x01\x00\x00\x00").unwrap();
        assert!(open_warning(&bin, 1 << 20).unwrap().contains("binary"));
        let utf16 = dir.join("u.txt");
        std::fs::write(&utf16, "plain text\r\n".encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<u8>>()).unwrap();
        assert_eq!(open_warning(&utf16, 1 << 20), None, "UTF-16 has NULs but isn't binary");
        assert_eq!(open_warning(&dir.join("new.txt"), 0), None, "a new file just opens");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn large_files_open_plain_and_say_so() {
        let dir = std::env::temp_dir().join(format!("strata-large-mode-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("big.rs");
        std::fs::write(&path, "fn main() {}\n".repeat(100)).unwrap();
        let mut app = StrataApp::new(None, None);
        app.settings.large_file_mb = 0.001;
        app.open_path(path.clone(), false);
        let d = &app.docs[app.active];
        assert_eq!(d.highlighter.syntax_name(), "Plain Text");
        assert_eq!(d.large.as_ref().map(|l| l.syntax.as_str()), Some("Rust"));
        assert!(app.status.contains("choose Rust"), "{}", app.status);
        assert!(!app.wraps(app.active));
        app.docs[app.active].wrap = Some(true);
        assert!(app.wraps(app.active), "wrap can still be turned on");
        app.docs[app.active].large.as_mut().unwrap().longest_line = crate::document::MAX_WRAP_LINE + 1;
        assert!(!app.wraps(app.active), "but not with a giant line");

        // Under the limit: as usual.
        app.settings.large_file_mb = 20.0;
        app.close_tab(app.active);
        app.open_path(path, false);
        let d = &app.docs[app.active];
        assert_eq!((d.highlighter.syntax_name(), d.large.is_none()), ("Rust", true));
        std::fs::remove_dir_all(dir).ok();
    }
}
