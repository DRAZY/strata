//! Replace in files: the replacement engine and the background job that
//! rewrites files on disk (files open in a tab are edited in their buffer
//! by the app instead, see `app/replace_files.rs`).
//!
//! Disk writes are careful: a file is rewritten only if it still looks the
//! way the search saw it, in its own encoding (BOM included), keeping its
//! line endings, permissions and symlinks, through the same atomic write as
//! Save. The original bytes are kept so the whole replace can be undone.

use crate::buffer::Buffer;
use crate::find::Query;
use crate::search::{self, Stamp};
use regex::Regex;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};

/// Originals kept for Undo Replace in Files, at most (bytes).
pub const UNDO_BYTES: usize = 256 * 1024 * 1024;

/// Edits worked out some other way than a search (a note rename's link
/// updates, see `notes::rename`): for a file and its text, the byte ranges
/// to replace and their new text, in order.
pub trait Rewrite: Send + Sync {
    fn edits(&self, path: &Path, text: &str) -> Vec<(Range<usize>, String)>;
}

/// One query + replacement text, ready to apply.
#[derive(Clone)]
pub struct Replacer {
    re: Regex,
    template: String,
    expand: bool,
    /// Edits come from here instead of `re` (see `Rewrite`).
    rewrite: Option<Arc<dyn Rewrite>>,
}

impl Replacer {
    pub fn new(q: &Query, replacement: &str) -> Result<Self, String> {
        let re = q.build().map_err(|e| e.to_string().lines().last().unwrap_or("invalid regex").trim().to_owned())?;
        let template = if q.regex { sublime_template(replacement) } else { replacement.to_owned() };
        Ok(Replacer { re, template, expand: q.regex, rewrite: None })
    }

    /// A replacer whose edits come from `rewrite`.
    pub fn rewriting(rewrite: Arc<dyn Rewrite>) -> Self {
        // Never matches: only `rewrite` makes edits.
        let re = Regex::new(r"[^\s\S]").expect("valid regex");
        Replacer { re, template: String::new(), expand: false, rewrite: Some(rewrite) }
    }

    /// `apply` for the file at `path`.
    pub fn apply_to(&self, path: &Path, text: &str, eol: &str) -> (String, usize) {
        match &self.rewrite {
            Some(rw) => {
                let edits = rw.edits(path, text);
                (crate::notes::rename::apply(text, &edits), edits.len())
            }
            None => self.apply(text, eol),
        }
    }

    /// The search regex.
    pub fn regex(&self) -> &Regex {
        &self.re
    }

    /// The replacement for one match. In a regex replacement `\1`, `$1` and
    /// `${name}` are groups, `\t` a tab and `\n` becomes `eol` (the file's
    /// line ending); a literal replacement is used as written.
    pub fn expand(&self, caps: &regex::Captures, eol: &str) -> String {
        if !self.expand {
            return self.template.clone();
        }
        let mut s = String::new();
        caps.expand(&self.template, &mut s);
        s.replace('\n', eol)
    }

    /// Replacements in `text` as (byte range, new text), line by line like
    /// the search: matches never span a line break, empty matches are
    /// skipped. `eol` is what `\n` in a regex replacement becomes.
    pub fn edits(&self, text: &str, eol: &str) -> Vec<(Range<usize>, String)> {
        let mut out = Vec::new();
        for (offset, line) in search::lines_with_offsets(text) {
            for caps in self.re.captures_iter(line) {
                let m = caps.get(0).expect("group 0 always matches");
                if m.start() == m.end() {
                    continue;
                }
                out.push((offset + m.start()..offset + m.end(), self.expand(&caps, eol)));
            }
        }
        out
    }

    /// The same edits as char ranges (for a rope buffer).
    pub fn char_edits(&self, text: &str, eol: &str) -> Vec<(Range<usize>, String)> {
        let (mut byte, mut chr) = (0, 0);
        self.edits(text, eol)
            .into_iter()
            .map(|(r, s)| {
                chr += text[byte..r.start].chars().count();
                let start = chr;
                chr += text[r.clone()].chars().count();
                byte = r.end;
                (start..chr, s)
            })
            .collect()
    }

    /// `text` with every replacement made, and how many there were.
    pub fn apply(&self, text: &str, eol: &str) -> (String, usize) {
        let edits = self.edits(text, eol);
        let mut out = String::with_capacity(text.len());
        let mut at = 0;
        for (r, s) in &edits {
            out.push_str(&text[at..r.start]);
            out.push_str(s);
            at = r.end;
        }
        out.push_str(&text[at..]);
        (out, edits.len())
    }
}

/// Sublime-style replacement text → `regex` syntax: `\1`…`\9` become
/// `${1}`…, `\n`/`\t` a newline/tab, `\\` a backslash and `\$` a dollar.
/// `$1` and `${name}` pass through.
pub fn sublime_template(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.peek().copied() {
            Some(d @ '0'..='9') => {
                chars.next();
                out.push_str(&format!("${{{d}}}"));
            }
            Some('n') => {
                chars.next();
                out.push('\n');
            }
            Some('t') => {
                chars.next();
                out.push('\t');
            }
            Some('\\') => {
                chars.next();
                out.push('\\');
            }
            Some('$') => {
                chars.next();
                out.push_str("$$");
            }
            _ => out.push('\\'),
        }
    }
    out
}

/// A file to rewrite on disk: the search saw `expected` matches in it when
/// it looked like `stamp`.
#[derive(Clone, Debug)]
pub struct Job {
    pub path: PathBuf,
    pub stamp: Option<Stamp>,
    pub expected: usize,
}

/// What happened to one file.
pub enum Outcome {
    Replaced { path: PathBuf, count: usize, original: Vec<u8>, after: After },
    /// An undo ran (see `swap`); refused, it hands the kept originals back.
    Undone(crate::swap::Outcome, Vec<Kept>),
    Skipped { path: PathBuf, reason: String },
}

/// Rewrite one file, or say why not. Returns (replacements, original bytes).
pub fn replace_file(job: &Job, r: &Replacer) -> Result<(usize, Vec<u8>), String> {
    const CHANGED: &str = "changed since the search";
    let path = &job.path;
    let Some(stamp) = job.stamp else { return Err("had unsaved changes in a tab that has since closed".into()) };
    let file = search::read_text(path).map_err(|e| e.to_string())?;
    if stamp != file.stamp {
        return Err(CHANGED.into());
    }
    if file.lossy {
        return Err(format!("not valid {} (saving would lose bytes)", file.encoding.name()));
    }
    if std::fs::metadata(path).is_ok_and(|m| m.permissions().readonly()) {
        return Err("read-only".into());
    }
    let crlf = file.text.find('\n').is_some_and(|i| file.text[..i].ends_with('\r'));
    let eol = if crlf { "\r\n" } else { "\n" };
    let (text, count) = r.apply_to(path, &file.text, eol);
    // Same count as the search saw, or the file isn't what the user confirmed.
    if count != job.expected || count == 0 {
        return Err(CHANGED.into());
    }
    let buffer = Buffer::from_str(&text);
    buffer.check_encodable(file.encoding)?;
    // Last look right before writing, to keep the race window tiny.
    if Stamp::of(path).ok() != Some(file.stamp) {
        return Err(CHANGED.into());
    }
    buffer.write_to(path, file.encoding).map_err(|e| format!("couldn't write: {e}"))?;
    Ok((count, file.bytes))
}

/// What a file looked like right after the replace wrote it: its size and
/// a fingerprint of every byte (see `swap::identity`); an undo puts the old
/// text back only over exactly that.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct After(Option<(u64, u64)>);

impl After {
    fn of(path: &Path) -> After {
        After(crate::swap::identity(path))
    }
}

pub struct Kept {
    path: PathBuf,
    original: Arc<Vec<u8>>,
    after: After,
}

/// Why an undo didn't restore a file, for a report.
fn describe(state: &crate::swap::State) -> String {
    match state {
        crate::swap::State::Restored => "restored".into(),
        crate::swap::State::Unchanged => "left as it was".into(),
        crate::swap::State::KeptAt(p) => format!("couldn't go back; it's kept at {}", p.display()),
    }
}

/// The background disk job (replace, or undo of the last replace) and its
/// running totals.
#[derive(Default)]
pub struct DiskReplace {
    rx: Option<Receiver<Outcome>>,
    cancel: Arc<AtomicBool>,
    done: Arc<AtomicUsize>,
    /// The worker thread has sent everything.
    finished: Arc<AtomicBool>,
    pub total: usize,
    pub undoing: bool,
    /// Occurrences replaced and files written (or restored, when undoing).
    pub replaced: usize,
    pub files: usize,
    pub skipped: Vec<(PathBuf, String)>,
    kept: Vec<Kept>,
    kept_bytes: usize,
    /// Files written whose originals were too big to keep for undo.
    pub not_undoable: usize,
    /// Most bytes of originals kept for undo (None: `UNDO_BYTES`); kept across jobs.
    pub limit: Option<usize>,
    /// How the last undo went.
    pub undo: Option<crate::swap::Outcome>,
    /// Tests: called with each file the undo has checked, just before it's put back.
    #[cfg(test)]
    pub undo_hook: Option<UndoHook>,
}

#[cfg(test)]
pub type UndoHook = Arc<dyn Fn(&Path) + Send + Sync>;

impl DiskReplace {
    pub fn running(&self) -> bool {
        self.rx.is_some()
    }

    pub fn progress(&self) -> (usize, usize) {
        (self.done.load(Ordering::Relaxed), self.total)
    }

    /// Files Undo Replace in Files would restore.
    pub fn undoable(&self) -> usize {
        self.kept.len()
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// Rewrite `jobs` off the UI thread. Forgets the previous undo.
    pub fn start(&mut self, jobs: Vec<Job>, r: Replacer, ctx: Option<egui::Context>) {
        *self = DiskReplace { total: jobs.len(), limit: self.limit, ..Default::default() };
        self.spawn(ctx, move |job: &Job| match replace_file(job, &r) {
            Ok((count, original)) => Outcome::Replaced { path: job.path.clone(), count, original, after: After::of(&job.path) },
            Err(reason) => Outcome::Skipped { path: job.path.clone(), reason },
        }, jobs);
    }

    /// Restore the files the last replace wrote.
    pub fn start_undo(&mut self, ctx: Option<egui::Context>) {
        self.start_restore(Vec::new(), None, ctx);
    }

    /// Restore the files the last replace wrote, and `extra` (more files, a
    /// rename back), all or nothing, off the UI thread (see `swap`). Files
    /// inside `top` are reached from it without following links; others
    /// from their own folder.
    pub fn start_restore(&mut self, extra: Vec<crate::swap::Item>, top: Option<PathBuf>, ctx: Option<egui::Context>) {
        let kept = std::mem::take(&mut self.kept);
        #[cfg(test)]
        let hook = self.undo_hook.clone();
        *self = DiskReplace { total: 1, undoing: true, limit: self.limit, ..Default::default() };
        let batch = vec![(kept, extra)];
        self.spawn(ctx, move |(kept, extra): &(Vec<Kept>, Vec<crate::swap::Item>)| {
            let top_of = |p: &Path| top.clone().filter(|t| p.starts_with(t)).unwrap_or_else(|| p.parent().map(Path::to_owned).unwrap_or_default());
            let mut items: Vec<crate::swap::Item> = Vec::new();
            for k in kept {
                // Never over a file whose state after the replace wasn't known.
                let expected = k.after.0.unwrap_or((u64::MAX, 0));
                let mut item = crate::swap::Item::restore(top_of(&k.path), k.path.clone(), k.original.clone(), expected);
                // The same file renamed back: its old text goes in under the old name.
                if let Some(back) = extra.iter().find(|e| e.original.is_none() && e.path == k.path) {
                    item.install_as = back.install_as.clone();
                }
                items.push(item);
            }
            items.extend(extra.iter().filter(|e| e.original.is_some() || !kept.iter().any(|k| k.path == e.path)).cloned());
            let paths: Vec<PathBuf> = items.iter().map(|i| i.path.clone()).collect();
            #[cfg(test)]
            let mut step = |s: crate::swap::Step| {
                if let (crate::swap::Step::BeforeInstall(i), Some(h)) = (s, &hook) {
                    h(&paths[i]);
                }
            };
            #[cfg(not(test))]
            let mut step = |_: crate::swap::Step| {
                let _ = &paths;
            };
            let out = crate::swap::restore_all_with(&items, &mut step);
            let back = if matches!(out, crate::swap::Outcome::Refused(..)) { kept.iter().map(|k| Kept { path: k.path.clone(), original: k.original.clone(), after: k.after }).collect() } else { Vec::new() };
            Outcome::Undone(out, back)
        }, batch);
    }

    fn spawn<T: Send + 'static>(&mut self, ctx: Option<egui::Context>, work: impl Fn(&T) -> Outcome + Send + 'static, items: Vec<T>) {
        let (tx, rx) = channel();
        self.rx = Some(rx);
        let (cancel, done, finished) = (self.cancel.clone(), self.done.clone(), self.finished.clone());
        std::thread::spawn(move || {
            for item in &items {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let outcome = work(item);
                done.fetch_add(1, Ordering::Relaxed);
                if tx.send(outcome).is_err() {
                    break;
                }
                if let Some(ctx) = &ctx {
                    ctx.request_repaint();
                }
            }
            drop(tx);
            finished.store(true, Ordering::Release);
            if let Some(ctx) = &ctx {
                ctx.request_repaint();
            }
        });
    }

    /// Collect finished files. Returns true on the call where the job ends.
    pub fn poll(&mut self) -> bool {
        let Some(rx) = &self.rx else { return false };
        loop {
            match rx.try_recv() {
                Ok(Outcome::Replaced { path, count, original, after }) => {
                    self.replaced += count;
                    self.files += 1;
                    if self.kept_bytes + original.len() <= self.limit.unwrap_or(UNDO_BYTES) {
                        self.kept_bytes += original.len();
                        self.kept.push(Kept { path, original: Arc::new(original), after });
                    } else {
                        self.not_undoable += 1;
                    }
                }
                Ok(Outcome::Undone(out, back)) => {
                    use crate::swap::{Outcome as O, State};
                    match &out {
                        O::Done(states) => self.files = states.iter().filter(|(_, s)| *s == State::Restored).count(),
                        O::Refused(why, kept) => {
                            self.skipped.extend(why.iter().cloned());
                            self.skipped.extend(kept.iter().map(|(p, s)| (p.clone(), describe(s))));
                            // Nothing changed: the undo can be tried again.
                            self.kept_bytes = back.iter().map(|k| k.original.len()).sum();
                            self.kept = back;
                        }
                        O::Partial(states, why) => {
                            self.files = states.iter().filter(|(_, s)| *s == State::Restored).count();
                            self.skipped.extend(states.iter().filter(|(_, s)| *s != State::Restored).map(|(p, s)| (p.clone(), describe(s))));
                            self.skipped.push((PathBuf::new(), why.clone()));
                        }
                    }
                    self.undo = Some(out);
                }
                Ok(Outcome::Skipped { path, reason }) => self.skipped.push((path, reason)),
                Err(TryRecvError::Empty) => return false,
                Err(TryRecvError::Disconnected) => {
                    self.rx = None;
                    let (done, total) = self.progress();
                    if done < total {
                        self.skipped.push((PathBuf::new(), format!("stopped: {} files not processed", total - done)));
                    }
                    return true;
                }
            }
        }
    }

    /// Block until the worker is done (tests and dev hooks); `poll` then
    /// collects the outcomes.
    pub fn wait_until_done(&self) {
        while self.running() && !self.finished.load(Ordering::Acquire) {
            std::thread::yield_now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoding::Encoding;
    use std::fs;

    fn regex(text: &str) -> Query {
        Query { text: text.into(), regex: true, case_sensitive: true, whole_word: false }
    }

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("strata-replace-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn job(path: &Path, expected: usize) -> Job {
        Job { path: path.to_owned(), stamp: Some(Stamp::of(path).unwrap()), expected }
    }

    #[test]
    fn expansion_supports_dollar_and_backslash_groups() {
        let r = Replacer::new(&regex(r"(\w+)@(?<host>\w+)"), r"\2 at $1 (${host}) \\1 \$1").unwrap();
        assert_eq!(r.apply("x a@b y", "\n").0, r"x b at a (b) \1 $1 y");
        // `${1}x` stays group 1 followed by x; \1x too.
        let r = Replacer::new(&regex(r"(a)"), r"\1x-${1}x").unwrap();
        assert_eq!(r.apply("a", "\n").0, "ax-ax");
        // \n becomes the file's line ending.
        let r = Replacer::new(&regex(r"(a);(b)"), r"$1\n$2").unwrap();
        assert_eq!(r.apply("a;b\r\n", "\r\n").0, "a\r\nb\r\n");
    }

    #[test]
    fn literal_mode_takes_replacement_verbatim_and_respects_options() {
        let q = Query { text: "a.b".into(), case_sensitive: false, ..Default::default() };
        let r = Replacer::new(&q, "$1\\1").unwrap();
        assert_eq!(r.apply("A.B axb a.b", "\n"), ("$1\\1 axb $1\\1".into(), 2));
        let q = Query { text: "cat".into(), whole_word: true, case_sensitive: true, ..Default::default() };
        assert_eq!(Replacer::new(&q, "dog").unwrap().apply("cat concat cat", "\n"), ("dog concat dog".into(), 2));
    }

    #[test]
    fn matches_stay_within_lines_and_char_edits_line_up() {
        let r = Replacer::new(&regex(r"\s+$"), "").unwrap();
        assert_eq!(r.apply("a  \r\nb \n", "\n"), ("a\r\nb\n".into(), 2), "line breaks are never matched");
        let r = Replacer::new(&Query::literal("é", false), "e").unwrap();
        assert_eq!(r.char_edits("😀é\né", "\n"), vec![(1..2, "e".into()), (3..4, "e".into())]);
    }

    #[test]
    fn disk_replace_preserves_encoding_bom_and_line_endings() {
        let d = dir("enc");
        let wide = d.join("wide.txt");
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend("héllo world\r\nworld\r\n".encode_utf16().flat_map(u16::to_le_bytes));
        fs::write(&wide, &bytes).unwrap();
        let crlf = d.join("crlf.txt");
        fs::write(&crlf, "world\r\nsecond world\r\n").unwrap();
        let latin = d.join("latin.txt");
        fs::write(&latin, b"caf\xE9 world\n").unwrap();
        let r = Replacer::new(&regex(r"w(orld)"), r"W\1").unwrap();
        for (p, n) in [(&wide, 2), (&crlf, 2), (&latin, 1)] {
            assert_eq!(replace_file(&job(p, n), &r).unwrap().0, n);
        }
        let mut expect = vec![0xFF, 0xFE];
        expect.extend("héllo World\r\nWorld\r\n".encode_utf16().flat_map(u16::to_le_bytes));
        assert_eq!(fs::read(&wide).unwrap(), expect);
        assert_eq!(fs::read(&crlf).unwrap(), b"World\r\nsecond World\r\n");
        assert_eq!(fs::read(&latin).unwrap(), b"caf\xE9 World\n");
        assert_eq!(search::read_text(&wide).unwrap().encoding, Encoding::UTF16LE_BOM);
        fs::remove_dir_all(d).ok();
    }

    #[test]
    fn skips_binary_changed_lossy_and_unencodable_files() {
        let d = dir("skip");
        let r = Replacer::new(&Query::literal("x", false), "y").unwrap();
        let bin = d.join("bin.dat");
        fs::write(&bin, b"x\0x").unwrap();
        assert_eq!(replace_file(&job(&bin, 2), &r).unwrap_err(), "binary file");

        let changed = d.join("changed.txt");
        fs::write(&changed, "x x").unwrap();
        let stale = job(&changed, 2);
        fs::write(&changed, "x x x").unwrap();
        assert!(replace_file(&stale, &r).unwrap_err().contains("changed"));
        // Same size, different matches (mtime may not have ticked): the count catches it.
        fs::write(&changed, "x - -").unwrap();
        let mut same_len = job(&changed, 2);
        same_len.stamp = Some(Stamp::of(&changed).unwrap());
        assert!(replace_file(&same_len, &r).unwrap_err().contains("changed"));
        assert_eq!(fs::read_to_string(&changed).unwrap(), "x - -", "untouched");

        let lossy = d.join("lossy.txt");
        fs::write(&lossy, b"\xEF\xBB\xBFx \xFF").unwrap(); // UTF-8 BOM, then invalid UTF-8
        assert!(replace_file(&job(&lossy, 1), &r).unwrap_err().contains("not valid"));
        assert_eq!(fs::read(&lossy).unwrap(), b"\xEF\xBB\xBFx \xFF");

        let latin = d.join("latin.txt");
        fs::write(&latin, b"x \xE9").unwrap();
        let to_cjk = Replacer::new(&Query::literal("x", false), "漢").unwrap();
        assert!(replace_file(&job(&latin, 1), &to_cjk).unwrap_err().contains("can't be saved"));
        assert_eq!(fs::read(&latin).unwrap(), b"x \xE9");
        fs::remove_dir_all(d).ok();
    }

    #[cfg(unix)]
    #[test]
    fn keeps_symlinks_and_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let d = dir("link");
        let real = d.join("real.sh");
        fs::write(&real, "echo x\n").unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o755)).unwrap();
        let link = d.join("link.sh");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let r = Replacer::new(&Query::literal("x", false), "y").unwrap();
        replace_file(&job(&link, 1), &r).unwrap();
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(fs::read_to_string(&real).unwrap(), "echo y\n");
        assert_eq!(fs::metadata(&real).unwrap().permissions().mode() & 0o777, 0o755);
        fs::remove_dir_all(d).ok();
    }

    /// A sync tool writes the file after the undo checked it but before it
    /// puts the old text back: the newer content must survive.
    #[test]
    fn undo_never_overwrites_a_write_that_lands_after_its_check() {
        let d = dir("undo-race");
        let a = d.join("a.txt");
        fs::write(&a, "one x\n").unwrap();
        let mut task = DiskReplace::default();
        task.start(vec![job(&a, 1)], Replacer::new(&Query::literal("x", false), "y").unwrap(), None);
        task.wait_until_done();
        assert!(task.poll());
        let target = a.clone();
        task.undo_hook = Some(Arc::new(move |p: &Path| {
            if p == target {
                fs::write(p, "the sync tool's newer text\n").unwrap();
            }
        }));
        task.start_undo(None);
        task.wait_until_done();
        task.poll();
        assert_eq!(fs::read_to_string(&a).unwrap(), "the sync tool's newer text\n");
        // Strata's own version is kept aside, marked, and the report says where.
        assert!(matches!(task.undo, Some(crate::swap::Outcome::Partial(..))));
        assert!(task.skipped.iter().any(|(_, why)| why.contains("kept at")), "{:?}", task.skipped);
        fs::remove_dir_all(d).ok();
    }

    #[test]
    fn background_job_reports_and_undoes() {
        let d = dir("job");
        let (a, b) = (d.join("a.txt"), d.join("b.txt"));
        fs::write(&a, "one x\n").unwrap();
        fs::write(&b, "two x x\n").unwrap();
        let gone = Job { path: d.join("gone.txt"), stamp: Some(Stamp { len: 1, mtime: None }), expected: 1 };
        let jobs = vec![job(&a, 1), job(&b, 2), gone];
        let mut task = DiskReplace::default();
        task.start(jobs, Replacer::new(&Query::literal("x", false), "y").unwrap(), None);
        task.wait_until_done();
        assert!(task.poll());
        assert_eq!((task.replaced, task.files, task.skipped.len(), task.undoable()), (3, 2, 1, 2));
        assert_eq!(fs::read_to_string(&b).unwrap(), "two y y\n");

        // b changes after the replace: the undo is all or nothing, so nothing
        // is restored, b is named, and it can be tried again.
        fs::write(&b, "edited later\n").unwrap();
        task.start_undo(None);
        task.wait_until_done();
        assert!(task.poll());
        assert_eq!(fs::read_to_string(&a).unwrap(), "one y\n");
        assert_eq!(fs::read_to_string(&b).unwrap(), "edited later\n");
        assert_eq!((task.files, task.skipped.len(), task.undoable()), (0, 1, 2));
        assert_eq!(task.skipped[0].0, b);
        // b back as the replace left it: now everything comes back.
        fs::write(&b, "two y y\n").unwrap();
        task.start_undo(None);
        task.wait_until_done();
        assert!(task.poll());
        assert_eq!(fs::read_to_string(&a).unwrap(), "one x\n");
        assert_eq!(fs::read_to_string(&b).unwrap(), "two x x\n");
        assert_eq!((task.files, task.skipped.len(), task.undoable()), (2, 0, 0));
        fs::remove_dir_all(d).ok();
    }
}
