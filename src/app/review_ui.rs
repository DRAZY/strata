//! Review Changes, the app half: the Changes list in the side bar, the
//! review tab (a Compare tab of a changed file against its base, with Keep
//! and Revert), the commands, and the count in the status bar.
//!
//! Reverting is an edit in the file's buffer, undoable like any other, and
//! reaches the disk when you save. Keeping changes nothing on disk: the kept
//! text becomes that file's base, so the change stops showing. A new file is
//! reverted by moving it to the Trash (after asking); a deleted one by
//! putting its text back in a tab, to save.

use super::StrataApp;
use crate::encoding::Encoding;
use crate::review::{self, Base, FileChange, Outcome, Scan, Scanner, Status};
use crate::sidebar::Tab;
use egui::{Color32, RichText};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Rescan this often (seconds) while the list or a review tab is up, or
/// four times as long as the last scan took, whichever is longer.
const RESCAN: f64 = 2.0;

/// A renamed file's old path, text and encoding, put back in a tab.
type Restore = (PathBuf, String, Encoding);

/// "Move the new file to the Trash?" before reverting a new or renamed file.
struct Confirm {
    path: PathBuf,
    /// A renamed file: its old path, text and encoding, put back in a tab.
    restore: Option<Restore>,
    /// The file as listed when asked: what's trashed must still be it.
    listed: Option<review::Stat>,
    hash: u64,
}

impl Confirm {
    fn of(f: &FileChange, restore: Option<Restore>) -> Self {
        Self { path: f.path.clone(), restore, listed: f.stat, hash: f.hash }
    }
}

/// A file's base, as far as it's known.
enum BaseText {
    /// Being read (`Scanner::base`); the review tab opens or follows when it arrives.
    Pending,
    /// The text and the encoding it was read with.
    Ready(String, Encoding),
    Failed(String),
}

#[derive(Default)]
pub(super) struct ReviewState {
    scanner: Option<Scanner>,
    pub base: Base,
    /// The last finished scan.
    pub scan: Option<Scan>,
    pub error: Option<String>,
    /// A scan or Start Review's snapshot is on its way.
    waiting: bool,
    starting: bool,
    generation: u64,
    /// Scans asked for before the base last changed are dropped.
    min_generation: u64,
    /// When to scan again (input time).
    next_scan: f64,
    now: f64,
    /// Files ticked off, with the fingerprint of the bytes then (None: take
    /// the next scan's). A file that changes again loses its tick.
    pub reviewed: HashMap<PathBuf, Option<u64>>,
    /// Text kept per file (Keep): the base those files are compared with.
    pub kept: HashMap<PathBuf, Arc<String>>,
    /// The Reviewed section of the list is open.
    show_reviewed: bool,
    confirm: Option<Confirm>,
    /// The base changed: review tabs take the new one with the next scan.
    stale_tabs: bool,
    /// Base bytes read for files under review (None: not in the base),
    /// for the scan's commit; and the files asked about.
    bases: HashMap<PathBuf, Option<Arc<Vec<u8>>>>,
    asked: std::collections::HashSet<PathBuf>,
    /// Open this file's review tab once its base arrives.
    pub pending_open: Option<PathBuf>,
    /// A file on its way to the Trash (with a renamed file's old path to reopen).
    pub pending_trash: Option<(PathBuf, Option<Restore>)>,
    /// The encoding each review tab's base was read with (Revert File and
    /// restored files use it, so the bytes come back as they were).
    base_enc: HashMap<PathBuf, Encoding>,
}

impl ReviewState {
    /// The revert question is up.
    pub fn confirming(&self) -> bool {
        self.confirm.is_some()
    }

    pub fn reviewing(&self) -> bool {
        matches!(self.base, Base::Snapshot(_))
    }

    /// Ticked off, or every change kept.
    pub fn is_done(&self, f: &FileChange) -> bool {
        f.kept || self.reviewed.contains_key(&f.path)
    }

    fn file(&self, path: &Path) -> Option<&FileChange> {
        self.scan.as_ref()?.files.iter().find(|f| f.path == path)
    }
}

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

fn status_color(s: Status, light: bool) -> Color32 {
    let (dark, lite) = match s {
        Status::Modified => (Color32::from_rgb(0x6c, 0xa8, 0xf0), Color32::from_rgb(0x1f, 0x5f, 0xb4)),
        Status::Added => (Color32::from_rgb(0x6c, 0xc0, 0x6c), Color32::from_rgb(0x2a, 0x7a, 0x2a)),
        Status::Deleted => (Color32::from_rgb(0xe8, 0x70, 0x70), Color32::from_rgb(0xb0, 0x30, 0x30)),
        Status::Renamed => (Color32::from_rgb(0xc0, 0x90, 0xe8), Color32::from_rgb(0x7a, 0x3c, 0xb0)),
    };
    if light { lite } else { dark }
}

/// What a click in the list asked for.
enum ListAction {
    Open(PathBuf),
    Reviewed(PathBuf, bool),
    KeepFile(PathBuf),
    RevertFile(PathBuf),
    Start,
    End,
    Rescan,
    ClearReviewed,
}

impl StrataApp {
    fn review_scanner(&mut self, ctx: Option<&egui::Context>) -> &Scanner {
        self.review.scanner.get_or_insert_with(|| Scanner::new(ctx.cloned()))
    }

    /// The folder whose repository is reviewed: the snapshot's, the first
    /// side bar folder, or the current file's.
    fn review_folder(&self) -> Option<PathBuf> {
        if let Base::Snapshot(s) = &self.review.base {
            return Some(s.top.clone());
        }
        self.sidebar.root().map(Path::to_path_buf).or_else(|| self.docs.get(self.active)?.path.as_deref()?.parent().map(Path::to_path_buf))
    }

    fn review_tabs_open(&self) -> bool {
        self.docs.iter().any(|d| d.compare.as_ref().is_some_and(|c| c.review.is_some()))
    }

    /// Scanning is worth it: the list is up, a review is on, or a review tab is open.
    fn review_live(&self) -> bool {
        (self.sidebar.shown() && self.sidebar.tab == Tab::Changes) || self.review.reviewing() || self.review_tabs_open()
    }

    fn review_request(&mut self, ctx: Option<&egui::Context>) {
        let Some(folder) = self.review_folder() else {
            self.review.error = Some("Open a folder or a file in a Git repository to see its changes".into());
            return;
        };
        self.review.generation += 1;
        let (base, kept, generation) = (self.review.base.clone(), self.review.kept.clone(), self.review.generation);
        self.review_scanner(ctx).scan(&folder, base, kept, generation);
        self.review.waiting = true;
    }

    /// Each frame: collect results and rescan when it's time.
    pub(super) fn review_frame(&mut self, ctx: &egui::Context, now: f64) {
        self.review.now = now;
        if self.review.scanner.is_none() && !self.review_live() {
            return;
        }
        self.review_poll();
        if self.review_live() && !self.review.waiting && !self.review.starting && now >= self.review.next_scan {
            self.review_request(Some(ctx));
        }
        if self.review_live() {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(RESCAN));
        }
        self.review_confirm_ui(ctx);
    }

    /// Take finished scans and snapshots. True if one arrived.
    pub(super) fn review_poll(&mut self) -> bool {
        let mut got = false;
        while let Some(outcome) = self.review.scanner.as_ref().and_then(Scanner::poll) {
            got = true;
            match outcome {
                Outcome::Scanned(generation, result) => {
                    // The worker skips requests replaced before it got to them.
                    if generation == self.review.generation {
                        self.review.waiting = false;
                    }
                    if generation < self.review.min_generation {
                        continue;
                    }
                    match result {
                        Ok(scan) => self.review_got(scan),
                        Err(e) => {
                            self.review.error = Some(e);
                            self.review.scan = None;
                            self.review.next_scan = self.review.now + RESCAN;
                        }
                    }
                }
                Outcome::Snapshot(result) => {
                    self.review.starting = false;
                    match result {
                        Ok(snap) => {
                            let already = snap.files.len();
                            self.review.base = Base::Snapshot(Arc::new(snap));
                            self.review_reset();
                            self.status = if already == 0 {
                                "Review started: changes from now on show in the Changes list".into()
                            } else {
                                format!("Review started: {} already changed count as the starting point", plural(already, "file"))
                            };
                        }
                        Err(e) => self.status = e,
                    }
                }
                Outcome::Trashed(path, result) => self.review_trashed(path, result),
                Outcome::Base(path, bytes) => {
                    if !self.review.asked.remove(&path) {
                        continue; // asked for an earlier base
                    }
                    self.review.bases.insert(path.clone(), bytes.map(Arc::new));
                    if self.review.pending_open.as_ref() == Some(&path) {
                        self.review.pending_open = None;
                        self.open_review(&path);
                    } else {
                        self.review_refresh_bases();
                    }
                }
            }
        }
        got
    }

    fn review_got(&mut self, scan: Scan) {
        let moved = self.review.scan.as_ref().is_some_and(|old| old.commit != scan.commit || old.top != scan.top);
        let by_path: HashMap<&PathBuf, &FileChange> = scan.files.iter().map(|f| (&f.path, f)).collect();
        // A tick lasts while the file stays as it was when ticked.
        self.review.reviewed.retain(|p, hash| match by_path.get(p) {
            Some(f) => *hash.get_or_insert(f.hash) == f.hash,
            None => false,
        });
        self.review.kept.retain(|p, _| by_path.contains_key(p));
        self.review.next_scan = self.review.now + RESCAN.max(scan.took.as_secs_f64() * 4.0);
        self.review.scan = Some(scan);
        self.review.error = None;
        if moved {
            self.review_forget_bases();
        }
        if moved || std::mem::take(&mut self.review.stale_tabs) {
            self.review_refresh_bases();
        }
    }

    /// The base changed (Start Review, End Review): forget ticks and kept
    /// text, drop scans on their way, scan again now.
    fn review_reset(&mut self) {
        self.review.reviewed.clear();
        self.review.kept.clear();
        self.review.min_generation = self.review.generation + 1;
        self.review.scan = None;
        self.review.waiting = false;
        self.review.next_scan = 0.0;
        self.review.stale_tabs = true;
        self.review_forget_bases();
    }

    /// Bases read for another commit or review don't apply.
    fn review_forget_bases(&mut self) {
        self.review.bases.clear();
        self.review.asked.clear();
        self.review.pending_open = None;
    }

    /// The base's bytes for `path` (None: not in the base), or Pending while
    /// the scanner reads them: git can be slow, so never on this thread.
    fn review_base_bytes(&mut self, path: &Path) -> Result<Option<Option<Vec<u8>>>, String> {
        let Some(scan) = self.review.scan.as_ref() else { return Err("Looking for changes first…".into()) };
        let f = self.review.file(path);
        if f.is_some_and(|f| f.status == Status::Added) {
            return Ok(Some(None));
        }
        let rel = match f {
            Some(f) => f.old.as_ref().map_or(&f.rel, |o| &o.1).clone(),
            None => path.strip_prefix(&scan.top).ok().map(|r| r.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")).ok_or("Not in the repository")?,
        };
        if let Some(found) = review::base_now(&self.review.base, &rel) {
            return Ok(Some(found));
        }
        if let Some(found) = self.review.bases.get(path) {
            return Ok(Some(found.as_ref().map(|b| b.to_vec())));
        }
        if self.review.asked.insert(path.to_owned()) {
            let (top, commit, base) = (scan.top.clone(), scan.commit.clone(), self.review.base.clone());
            self.review_scanner(None).base(path, &top, base, commit, rel);
        }
        Ok(None)
    }

    /// The base's text for `path`, shown in tab `i`: kept text, or the
    /// base's bytes decoded as the tab reads its file (see
    /// `review::decode_base`), with the line breaks Git would check out.
    /// A file no longer in the list reads as the base has it.
    fn review_base(&mut self, path: &Path, i: usize) -> BaseText {
        let enc = self.docs[i].encoding;
        if let Some(k) = self.review.kept.get(path) {
            return BaseText::Ready(k.to_string(), self.review.base_enc.get(path).copied().unwrap_or(enc));
        }
        let in_list = self.review.file(path).is_some();
        let crlf = self.review.file(path).is_some_and(|f| f.crlf);
        let bytes = match self.review_base_bytes(path) {
            Err(e) => return BaseText::Failed(e),
            Ok(None) => return BaseText::Pending,
            Ok(Some(Some(b))) => b,
            Ok(Some(None)) if in_list && self.review.file(path).is_some_and(|f| f.status != Status::Added) => {
                return BaseText::Failed("Could not read the earlier version".into());
            }
            Ok(Some(None)) => Vec::new(),
        };
        match review::decode_base(&bytes, enc) {
            Ok((text, enc)) => BaseText::Ready(review::as_checked_out(text, crlf), enc),
            Err(e) => BaseText::Failed(e),
        }
    }

    /// The review tabs follow a new base (HEAD moved, or the review started or ended).
    fn review_refresh_bases(&mut self) {
        for i in 0..self.docs.len() {
            let Some(path) = self.docs[i].compare.as_ref().and_then(|c| c.review.clone()) else { continue };
            if let BaseText::Ready(text, enc) = self.review_base(&path, i) {
                self.review.base_enc.insert(path.clone(), enc);
                self.docs[i].compare.as_mut().expect("review tab").set_base(&text);
            }
        }
    }

    /// The current tab, if it's a review tab.
    fn review_tab(&self) -> Option<usize> {
        self.docs.get(self.active)?.compare.as_ref()?.review.as_ref()?;
        Some(self.active)
    }

    /// Files still to review, in list order.
    fn review_queue(&self) -> Vec<PathBuf> {
        self.review.scan.as_ref().map_or_else(Vec::new, |s| s.files.iter().filter(|f| !self.review.is_done(f)).map(|f| f.path.clone()).collect())
    }

    /// Files still to review, for the status bar; None when no review is in view.
    pub(super) fn review_remaining(&self) -> Option<usize> {
        let s = self.review.scan.as_ref()?;
        (self.review_live() && !s.files.is_empty()).then(|| s.files.iter().filter(|f| !self.review.is_done(f)).count())
    }

    /// Show `path`'s changes in a review tab: a Compare tab with the base on
    /// the left and the file, editable, on the right.
    pub(super) fn open_review(&mut self, path: &Path) -> bool {
        let Some(f) = self.review.file(path).cloned() else {
            self.status = format!("{} has no changes to review", path.display());
            return false;
        };
        if let Some(why) = &f.blocked {
            self.status = format!("{}: {why}", f.rel);
            return false;
        }
        if f.binary {
            self.status = format!("{} isn't text: Strata can't show its changes", f.rel);
            return false;
        }
        // Only files really inside the repository: no link may lead elsewhere.
        let top = self.review.scan.as_ref().map(|s| s.top.clone()).unwrap_or_default();
        if !review::inside(&top, path) || f.old.as_ref().is_some_and(|(old, _)| !review::inside(&top, old)) {
            self.status = format!("{} isn't inside the repository (a link leads elsewhere): Strata won't open it for review", f.rel);
            return false;
        }
        // Already open: go there.
        if let Some(i) = self.docs.iter().position(|d| d.compare.as_ref().is_some_and(|c| c.review.as_deref() == Some(path))) {
            self.activate(i);
            self.review_close_others(path);
            return true;
        }
        // The base first: reading it can take a moment (the tab opens when it's here).
        match self.review_base_bytes(path) {
            Err(e) => {
                self.status = format!("{}: {e}", f.rel);
                return false;
            }
            Ok(None) => {
                self.review.pending_open = Some(path.to_owned());
                self.status = format!("Reading the earlier version of {}…", f.rel);
                return false;
            }
            Ok(Some(_)) => {}
        }
        let opened = self.tab_for_path(path).is_none();
        if opened {
            self.open_path(path.to_owned(), false);
        }
        let Some(r) = self.docs.iter().position(|d| d.path.as_deref() == Some(path) && d.compare.is_none()) else { return false };
        if self.docs[r].binary.is_some() {
            self.status = format!("{} isn't text: Strata can't show its changes", f.rel);
            return false;
        }
        let (base, enc) = match self.review_base(path, r) {
            BaseText::Ready(text, enc) => (text, enc),
            BaseText::Pending => return false,
            BaseText::Failed(e) => {
                self.status = format!("{}: {e}", f.rel);
                if opened {
                    self.close_tab(r);
                }
                return false;
            }
        };
        self.review.base_enc.insert(path.to_owned(), enc);
        // A deleted file comes back as it was: its encoding and line breaks.
        if f.status == Status::Deleted && !self.docs[r].is_dirty() && self.docs[r].editor.buffer.len_chars() == 0 {
            let d = &mut self.docs[r];
            d.encoding = enc;
            d.encoding_guessed = false;
            d.editor.buffer.set_line_ending(crate::buffer::Buffer::from_str(&base).line_ending());
        }
        let plain = self.docs[r].view.id;
        let name = f.old.as_ref().map_or_else(|| self.docs[r].name(), |(p, _)| p.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned()));
        let label = format!("{name} ({})", if self.review.reviewing() { "review start" } else { "HEAD" });
        let syntax = self.docs[r].highlighter.syntax_name().to_owned();
        self.open_compare(r, &base, label, None, &syntax, None);
        let tab = self.active;
        self.docs[tab].compare.as_mut().expect("compare tab").review = Some(path.to_owned());
        // The plain tab was only opened to make this one: the review tab holds the buffer.
        if opened && let Some(k) = self.docs.iter().position(|d| d.view.id == plain) {
            self.close_tab(k);
        }
        self.review_close_others(path);
        let i = self.active;
        let d = &mut self.docs[i];
        let c = d.compare.as_mut().expect("review tab");
        let n = c.hunks().len();
        if n > 0 {
            c.go_to_hunk(&mut d.editor, 0);
        }
        self.status = match (f.status, n) {
            (_, 0) => format!("{}: no changes left in the editor", f.rel),
            (Status::Renamed, _) => format!("{} (renamed from {}): {}", f.rel, f.old.as_ref().map_or("", |o| o.1.as_str()), plural(n, "change")),
            _ => format!("{} ({}): {}", f.rel, f.status.label().to_lowercase(), plural(n, "change")),
        };
        true
    }

    /// One review tab at a time: others close, unless they have unsaved edits.
    fn review_close_others(&mut self, keep: &Path) {
        let mut k = 0;
        while k < self.docs.len() {
            let other = self.docs[k].compare.as_ref().and_then(|c| c.review.as_deref()).is_some_and(|p| p != keep);
            if other && (!self.docs[k].is_dirty() || self.other_view(k).is_some()) && self.close_tab(k) {
                continue;
            }
            k += 1;
        }
    }

    /// Review tab `i`, refreshed, the file it shows, and the change at the caret.
    fn review_target(&mut self, i: usize) -> Option<(PathBuf, Option<usize>)> {
        let d = &mut self.docs[i];
        let c = d.compare.as_mut()?;
        let path = c.review.clone()?;
        c.refresh(&mut d.editor, 0.0, true);
        Some((path, c.hunk_at_caret(&d.editor)))
    }

    fn review_name(path: &Path) -> String {
        path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
    }

    /// After Keep or Revert: go to the change now at `k`, and say what's left.
    fn review_after_edit(&mut self, i: usize, k: usize, done: &str, path: &Path) {
        let d = &mut self.docs[i];
        let c = d.compare.as_mut().expect("review tab");
        c.refresh(&mut d.editor, 0.0, true);
        let left = c.hunks().len();
        if left > 0 {
            c.go_to_hunk(&mut d.editor, k);
        }
        c.focus();
        // Line breaks of lines that matched all along show as no change.
        let breaks_differ = left == 0 && *d.editor.text_snapshot() != *c.base_text();
        let unsaved = if d.is_dirty() { " (unsaved)" } else { "" };
        self.status = if breaks_differ {
            format!("{done}. No changes left in {}, but some line endings still differ: Revert File puts them back{unsaved}", Self::review_name(path))
        } else if left == 0 {
            format!("{done}. No changes left in {}{unsaved}", Self::review_name(path))
        } else {
            format!("{done}. {} left in {}{unsaved}", plural(left, "change"), Self::review_name(path))
        };
        self.review.next_scan = 0.0;
    }

    /// Keep the change at the caret: it becomes part of the file's base.
    pub(super) fn review_keep_change(&mut self, i: usize) {
        let Some((path, k)) = self.review_target(i) else { return self.review_needs_tab() };
        let Some(k) = k else {
            self.status = "Put the caret on a change first (Review: Next Change)".into();
            return;
        };
        let d = &mut self.docs[i];
        let c = d.compare.as_mut().expect("review tab");
        let cur = d.editor.text_snapshot();
        let mut new = review::keep_edit(c.base_text(), &cur, &c.hunks()[k]);
        c.set_base(&new);
        c.refresh(&mut d.editor, 0.0, true);
        // The last change kept: the whole text is accepted, line breaks too.
        if c.hunks().is_empty() {
            new = cur.to_string();
            c.set_base(&new);
        }
        self.review.kept.insert(path.clone(), Arc::new(new));
        self.review_after_edit(i, k, "Kept", &path);
    }

    /// Revert the change at the caret: one undoable edit in the buffer.
    pub(super) fn review_revert_change(&mut self, i: usize) {
        let Some((path, k)) = self.review_target(i) else { return self.review_needs_tab() };
        let Some(k) = k else {
            self.status = "Put the caret on a change first (Review: Next Change)".into();
            return;
        };
        let d = &mut self.docs[i];
        let c = d.compare.as_mut().expect("review tab");
        let cur = d.editor.text_snapshot();
        let (range, text) = review::revert_edit(c.base_text(), &cur, &c.hunks()[k]);
        d.editor.replace_ranges(vec![(range, text)]);
        self.sync_views();
        self.review_after_edit(i, k, "Reverted", &path);
    }

    /// Keep every change in the file.
    pub(super) fn review_keep_file(&mut self, i: usize) {
        let Some((path, _)) = self.review_target(i) else { return self.review_needs_tab() };
        let d = &mut self.docs[i];
        let text = d.editor.text_snapshot();
        d.compare.as_mut().expect("review tab").set_base(&text);
        self.review.kept.insert(path.clone(), Arc::new(text.to_string()));
        self.review_after_edit(i, 0, "Kept every change", &path);
    }

    /// Put the whole file back as it was in the base. A new file goes to the
    /// Trash and a renamed one moves back (both after asking).
    pub(super) fn review_revert_file(&mut self, i: usize) {
        let Some((path, _)) = self.review_target(i) else { return self.review_needs_tab() };
        let Some(f) = self.review.file(&path).cloned() else {
            self.status = format!("{} has no changes to revert", Self::review_name(&path));
            return;
        };
        if !self.review_path_ok(&f) {
            return;
        }
        let enc = self.review.base_enc.get(&path).copied().unwrap_or(self.docs[i].encoding);
        match (f.status, f.old.clone()) {
            (Status::Renamed, _) if f.alias => {
                self.status = format!("{} was only respelled (letter case or accents), so it's the same file: rename it back in the side bar, or with Git", f.rel);
                return;
            }
            (Status::Added, _) => {
                if let Some(twin) = self.review_twin(&path) {
                    self.status = format!("{} is the same file as {twin} under another spelling: rename it back in the side bar, or with Git", f.rel);
                } else {
                    self.review.confirm = Some(Confirm::of(&f, None));
                }
                return;
            }
            (Status::Renamed, Some((old, _))) => {
                let base = self.docs[i].compare.as_ref().expect("review tab").base_text().to_owned();
                self.review.confirm = Some(Confirm::of(&f, Some((old, base, enc))));
                return;
            }
            _ => {}
        }
        let base = self.docs[i].compare.as_ref().expect("review tab").base_text().to_owned();
        let d = &mut self.docs[i];
        if *d.editor.text_snapshot() != base {
            let len = d.editor.buffer.len_chars();
            d.editor.replace_ranges(vec![(0..len, base)]);
            d.editor.select_ranges(&[0..0], 0);
        }
        // The whole file back means its bytes too: the base's encoding (and BOM).
        for d in self.docs.iter_mut().filter(|d| d.path.as_ref() == Some(&path)) {
            d.set_encoding(enc);
        }
        self.sync_views();
        self.review_after_edit(i, 0, "Reverted the file", &path);
    }

    /// Before anything that writes or trashes: the file belongs to the
    /// repository the list is about, really inside it, and is one Strata can
    /// handle. Says why not in the status bar.
    fn review_path_ok(&mut self, f: &FileChange) -> bool {
        let Some(top) = self.review.scan.as_ref().map(|s| s.top.clone()) else { return false };
        let why = if let Some(why) = &f.blocked {
            Some(why.clone())
        } else if !review::inside(&top, &f.path) || f.old.as_ref().is_some_and(|(old, _)| !review::inside(&top, old)) {
            Some("it isn't inside the repository (a link leads elsewhere)".to_owned())
        } else {
            None
        };
        match why {
            Some(why) => {
                self.status = format!("{}: {why}", f.rel);
                false
            }
            None => f.path.starts_with(&top),
        }
    }

    /// A file in the base that is the same file on disk as `path` under
    /// another spelling (see `review::tracked_twin`).
    fn review_twin(&self, path: &Path) -> Option<String> {
        let scan = self.review.scan.as_ref()?;
        let at_start: Vec<&String> = match &self.review.base {
            Base::Snapshot(s) => s.files.iter().filter(|(_, b)| b.is_some()).map(|(r, _)| r).collect(),
            Base::Head => Vec::new(),
        };
        review::tracked_twin(&scan.top, path, scan.tracked.keys().chain(at_start))
    }

    fn review_needs_tab(&mut self) {
        self.status = "Open a file from the Changes list first (Review: Show Changes)".into();
    }

    /// The question before moving a new file to the Trash.
    fn review_confirm_ui(&mut self, ctx: &egui::Context) {
        let Some(c) = &self.review.confirm else { return };
        let (mut yes, mut no) = (false, false);
        let name = Self::review_name(&c.path);
        crate::fonts::ensure_cjk(ctx, &format!("{name}{}", c.restore.as_ref().map_or(String::new(), |(old, _, _)| Self::review_name(old))));
        egui::Modal::new(egui::Id::new("review-revert-confirm")).show(ctx, |ui| {
            ui.set_width(360.0);
            let trash = crate::trash::trash_name();
            match &c.restore {
                None => {
                    ui.label(RichText::new(format!("Move the new file “{name}” to the {trash}?")).strong());
                    ui.add_space(4.0);
                    ui.label(RichText::new(format!("It wasn't there before, so reverting it removes it. You can put it back from the {trash}.")).small().weak());
                }
                Some((old, _, _)) => {
                    ui.label(RichText::new(format!("Move “{name}” back to “{}”?", Self::review_name(old))).strong());
                    ui.add_space(4.0);
                    ui.label(RichText::new(format!("“{name}” goes to the {trash}, and the old file opens with its earlier text, unsaved: save it to finish.")).small().weak());
                }
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                yes = ui.button("Revert").clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter));
                no = ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape));
            });
        });
        if yes {
            self.review_confirmed();
        } else if no {
            self.review.confirm = None;
        }
    }

    /// The new file goes to the Trash; a renamed file's old path opens with its text.
    pub(super) fn review_confirmed(&mut self) {
        let Some(Confirm { path, restore, listed, hash }) = self.review.confirm.take() else { return };
        if self.review.pending_trash.is_some() {
            return; // one at a time
        }
        // Still the file the list showed, inside this repository, and not one
        // that is the same file as a tracked one under another spelling.
        let Some(f) = self.review.file(&path).cloned() else {
            self.status = format!("{} is no longer in the list: nothing was moved", Self::review_name(&path));
            return;
        };
        let renamed_from = restore.as_ref().map(|(old, _, _)| old.clone());
        if f.stat != listed || f.hash != hash || f.old.as_ref().map(|(p, _)| p.clone()) != renamed_from {
            self.status = format!("{} changed since it was listed; look again", f.rel);
            return;
        }
        if let Some(old) = &renamed_from
            && std::fs::symlink_metadata(old).is_ok()
        {
            self.status = format!("{} exists again: nothing was moved", Self::review_name(old));
            return;
        }
        if !matches!(f.status, Status::Added | Status::Renamed) || f.alias || !self.review_path_ok(&f) {
            if f.alias {
                self.status = format!("{} is the same file as before, only respelled: nothing was moved", f.rel);
            }
            return;
        }
        // A "new" file that is really a tracked one under another spelling
        // is the only copy: never trashed, whatever the list said.
        if let Some(twin) = self.review_twin(&path) {
            self.status = format!("{} is the same file as {twin}: nothing was moved", f.rel);
            return;
        }
        // Off this thread. The worker claims the file first and then checks
        // it's the one listed (see `claim`): what's trashed can't be swapped.
        let Some(top) = self.review.scan.as_ref().map(|s| s.top.clone()) else { return };
        let Some(expected) = review::expected(&f) else {
            self.status = format!("{} changed since it was listed; look again", f.rel);
            return;
        };
        self.review_scanner(None).trash(&path, &top, renamed_from, expected);
        self.review.pending_trash = Some((path.clone(), restore));
        self.status = format!("Moving {} to the {}…", Self::review_name(&path), crate::trash::trash_name());
    }

    /// The Trash's answer for `path`.
    fn review_trashed(&mut self, path: PathBuf, result: Result<(), crate::claim::Refused>) {
        use crate::claim::Refused;
        let restore = match self.review.pending_trash.take() {
            Some((p, restore)) if p == path => restore,
            other => {
                self.review.pending_trash = other;
                None
            }
        };
        self.review.next_scan = 0.0;
        let name = Self::review_name(&path);
        match result {
            Ok(()) => {}
            Err(Refused::Changed) => return self.status = format!("{name} changed since it was listed; look again"),
            Err(Refused::OldBack) => return self.status = format!("The old name of {name} exists again: nothing was moved"),
            Err(Refused::KeptAt(at)) => return self.status = format!("{name} changed and its name was taken meanwhile; nothing was deleted: it's in {}", at.display()),
            Err(Refused::Outside) => return self.status = format!("{name} isn't inside the repository (a link leads elsewhere): nothing was moved"),
            Err(Refused::Failed(e)) => return self.status = format!("Could not move {name} to the {}: {e}", crate::trash::trash_name()),
        }
        self.follow_delete(&path);
        self.status = format!("Moved {name} to the {}", crate::trash::trash_name());
        if let Some((old, text, enc)) = restore {
            if std::fs::symlink_metadata(&old).is_ok() {
                self.status = format!("Moved {name} to the {}; {} exists again, so it wasn't reopened", crate::trash::trash_name(), Self::review_name(&old));
                return;
            }
            self.open_path(old.clone(), false);
            let d = self.doc();
            if d.path.as_ref() == Some(&old) {
                // Its bytes as they were: encoding, BOM and line breaks.
                d.encoding = enc;
                d.encoding_guessed = false;
                d.editor.buffer.set_line_ending(crate::buffer::Buffer::from_str(&text).line_ending());
                let len = d.editor.buffer.len_chars();
                d.editor.replace_ranges(vec![(0..len, text)]);
                d.editor.select_ranges(&[0..0], 0);
                self.status = format!("Moved {} back to {} (unsaved: save to finish)", Self::review_name(&path), Self::review_name(&old));
            }
        }
        self.review.next_scan = 0.0;
    }

    /// Tick a file off (or bring it back).
    pub(super) fn review_set_reviewed(&mut self, path: &Path, on: bool) {
        if on {
            let hash = self.review.file(path).map(|f| f.hash);
            self.review.reviewed.insert(path.to_owned(), hash);
        } else {
            self.review.reviewed.remove(path);
        }
    }

    /// Review: Mark File as Reviewed, for the review tab: then on to the next file.
    pub(super) fn review_mark_current(&mut self) {
        let Some(i) = self.review_tab() else { return self.review_needs_tab() };
        let Some(path) = self.docs[i].compare.as_ref().and_then(|c| c.review.clone()) else { return };
        let on = !self.review.reviewed.contains_key(&path);
        self.review_set_reviewed(&path, on);
        if on {
            let left = self.review_queue().len();
            self.status = format!("Marked {} as reviewed; {} to review", Self::review_name(&path), plural(left, "file"));
            if left > 0 {
                self.review_step_file(true, false);
            }
        } else {
            self.status = format!("{} is back on the list to review", Self::review_name(&path));
        }
    }

    /// Next (or previous) file to review after the one in the review tab,
    /// wrapping around. `last_change`: land on its last change.
    pub(super) fn review_step_file(&mut self, forward: bool, last_change: bool) {
        if self.review.scan.is_none() {
            self.review_show();
            self.status = "Looking for changes…".into();
            return;
        }
        let Some(files) = self.review.scan.as_ref().map(|s| s.files.clone()) else { return };
        let current = self.review_tab().and_then(|i| self.docs[i].compare.as_ref()?.review.clone());
        let todo: Vec<&FileChange> = files.iter().filter(|f| !self.review.is_done(f) && !f.binary).collect();
        if todo.is_empty() {
            self.status = if files.is_empty() { "No changes to review".into() } else { "Nothing left to review".into() };
            return;
        }
        let pos = current.as_ref().and_then(|p| files.iter().position(|f| &f.path == p));
        // From a plain tab of a file to review, start with that file.
        let here = self.docs.get(self.active).and_then(|d| d.path.as_ref()).and_then(|p| todo.iter().find(|f| &f.path == p).copied());
        let pick = match pos {
            None => here.unwrap_or(if forward { todo[0] } else { todo[todo.len() - 1] }),
            Some(at) => {
                let order = |f: &&FileChange| files.iter().position(|g| g.path == f.path).unwrap_or(0);
                if forward {
                    todo.iter().find(|f| order(f) > at).unwrap_or(&todo[0])
                } else {
                    todo.iter().rev().find(|f| order(f) < at).unwrap_or(&todo[todo.len() - 1])
                }
            }
        };
        let path = pick.path.clone();
        if Some(&path) == current.as_ref() && !last_change {
            self.status = format!("{} is the only file left to review", Self::review_name(&path));
            return;
        }
        if self.open_review(&path) && last_change {
            let i = self.active;
            let d = &mut self.docs[i];
            if let Some(c) = d.compare.as_mut() {
                let n = c.hunks().len();
                if n > 0 {
                    c.go_to_hunk(&mut d.editor, n - 1);
                }
            }
        }
    }

    /// Next (or previous) change: within the review tab, then on into the
    /// next file to review.
    pub(super) fn review_step_change(&mut self, forward: bool) {
        let Some(i) = self.review_tab() else { return self.review_step_file(forward, !forward) };
        let d = &mut self.docs[i];
        let c = d.compare.as_mut().expect("review tab");
        c.refresh(&mut d.editor, 0.0, true);
        match c.next_hunk(&d.editor, forward) {
            Some(k) => {
                let (k, n) = c.go_to_hunk(&mut d.editor, k);
                c.focus();
                self.status = format!("Change {k} of {n}");
            }
            None => self.review_step_file(forward, !forward),
        }
    }

    /// Review: Show Changes.
    pub(super) fn review_show(&mut self) {
        self.sidebar.visible = true;
        self.sidebar.tab = Tab::Changes;
        self.review.next_scan = 0.0;
        if self.review_folder().is_none() {
            self.status = "Open a folder or a file in a Git repository to see its changes".into();
        }
    }

    /// Review: Start Review: changes from now on, against a snapshot.
    pub(super) fn review_start(&mut self, ctx: Option<&egui::Context>) {
        if self.review.starting {
            return;
        }
        let folder = match &self.review.base {
            // Starting again takes a fresh snapshot of the same repository.
            Base::Snapshot(s) => Some(s.top.clone()),
            Base::Head => self.review_folder(),
        };
        let Some(folder) = folder else {
            self.status = "Open a folder or a file in a Git repository to start a review".into();
            return;
        };
        self.review_scanner(ctx).snapshot(&folder);
        self.review.starting = true;
        self.review_show();
        self.status = "Starting the review…".into();
    }

    /// Review: End Review: back to the changes since the last commit.
    pub(super) fn review_end(&mut self) {
        if !self.review.reviewing() {
            self.status = "No review is running: the list shows the changes since the last commit".into();
            return;
        }
        self.review.base = Base::Head;
        self.review_reset();
        self.status = "Review ended: the list shows the changes since the last commit".into();
    }

    /// A file was saved: a ticked file keeps its tick through your own save.
    pub(super) fn review_saved(&mut self, path: &Path) {
        // The tick now covers exactly the bytes you saved, nothing written after.
        if let Some(hash) = self.review.reviewed.get_mut(path) {
            *hash = Some(review::file_hash(path));
        }
        self.review.next_scan = 0.0;
    }

    /// Run a Review command.
    pub(super) fn review_command(&mut self, ctx: &egui::Context, cmd: crate::commands::Cmd) {
        use crate::commands::Cmd;
        let tab = self.review_tab();
        match cmd {
            Cmd::ReviewStart => self.review_start(Some(ctx)),
            Cmd::ReviewEnd => self.review_end(),
            Cmd::ReviewShowChanges => self.review_show(),
            Cmd::ReviewNextChange => self.review_step_change(true),
            Cmd::ReviewPrevChange => self.review_step_change(false),
            Cmd::ReviewNextFile => self.review_step_file(true, false),
            Cmd::ReviewPrevFile => self.review_step_file(false, false),
            Cmd::ReviewMarkReviewed => self.review_mark_current(),
            Cmd::ReviewKeepChange | Cmd::ReviewRevertChange | Cmd::ReviewKeepFile | Cmd::ReviewRevertFile => {
                let Some(i) = tab else { return self.review_needs_tab() };
                match cmd {
                    Cmd::ReviewKeepChange => self.review_keep_change(i),
                    Cmd::ReviewRevertChange => self.review_revert_change(i),
                    Cmd::ReviewKeepFile => self.review_keep_file(i),
                    _ => self.review_revert_file(i),
                }
            }
            _ => {}
        }
    }

    /// For a file in the list but not open for review: open it, then act.
    fn review_on_file(&mut self, path: &Path, act: fn(&mut Self, usize)) {
        if self.open_review(path) {
            let i = self.active;
            act(self, i);
        }
    }

    // ---- the list ------------------------------------------------------

    /// The Changes tab of the side bar.
    pub(super) fn changes_list(&mut self, ui: &mut egui::Ui) {
        let action = self.changes_list_ui(ui);
        match action {
            Some(ListAction::Open(p)) => {
                self.open_review(&p);
            }
            Some(ListAction::Reviewed(p, on)) => self.review_set_reviewed(&p, on),
            Some(ListAction::KeepFile(p)) => self.review_on_file(&p, Self::review_keep_file),
            Some(ListAction::RevertFile(p)) => self.review_on_file(&p, Self::review_revert_file),
            Some(ListAction::Start) => self.review_start(Some(ui.ctx())),
            Some(ListAction::End) => self.review_end(),
            Some(ListAction::Rescan) => self.review.next_scan = 0.0,
            Some(ListAction::ClearReviewed) => {
                self.review.reviewed.clear();
                self.review.kept.clear();
                self.review_refresh_bases();
                self.review.next_scan = 0.0;
            }
            None => {}
        }
    }

    fn changes_list_ui(&mut self, ui: &mut egui::Ui) -> Option<ListAction> {
        let mut action = None;
        let weak = ui.visuals().weak_text_color();
        let small = crate::fonts::secondary_size(egui::TextStyle::Body.resolve(ui.style()).size);
        let (reviewing, starting) = (self.review.reviewing(), self.review.starting);
        let what = if reviewing { "Since Start Review" } else { "Since last commit" };
        let hint = if reviewing {
            "Changes made after you chose Start Review. End Review goes back to the changes since the last commit."
        } else {
            "Files that differ from the last commit (HEAD). Start Review shows only the changes made from then on."
        };
        let review_button = |ui: &mut egui::Ui| -> Option<ListAction> {
            if reviewing {
                ui.small_button("End Review").on_hover_text("Go back to the changes since the last commit").clicked().then_some(ListAction::End)
            } else {
                let start = ui.add_enabled(!starting, egui::Button::new("Start Review").small());
                start.on_hover_text("From now on, show only the changes made after this moment (for example by a coding agent)").clicked().then_some(ListAction::Start)
            }
        };
        // Heading, Start Review and ⟳ share a row when they fit; in a narrow
        // side bar the button gets a row of its own.
        let font = egui::TextStyle::Body.resolve(ui.style());
        let text_w = |s: &str| ui.painter().layout_no_wrap(s.to_owned(), font.clone(), Color32::WHITE).size().x;
        let pad = 2.0 * ui.spacing().button_padding.x + ui.spacing().item_spacing.x;
        let one_row = text_w(what) + text_w(if reviewing { "End Review" } else { "Start Review" }) + text_w("⟳") + 2.0 * pad <= ui.available_width();
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("⟳").on_hover_text("Look for changes now").clicked() {
                    action = Some(ListAction::Rescan);
                }
                if one_row && let Some(a) = review_button(ui) {
                    action = Some(a);
                }
                // The heading takes what's left (cut short with … if it must).
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.add(egui::Label::new(RichText::new(what).strong()).truncate()).on_hover_text(hint);
                });
            });
        });
        if !one_row && let Some(a) = ui.horizontal(review_button).inner {
            action = Some(a);
        }
        let Some(scan) = self.review.scan.as_ref() else {
            let text = match (&self.review.error, self.review.starting) {
                (_, true) => "Starting the review…".to_owned(),
                (Some(e), _) => e.clone(),
                (None, _) => "Looking for changes…".to_owned(),
            };
            ui.add_space(6.0);
            ui.label(RichText::new(text).color(weak));
            return action;
        };
        let files = scan.files.clone();
        let done = files.iter().filter(|f| self.review.is_done(f)).count();
        let filtered = scan.filtered;
        let mut summary = match (files.len(), done) {
            (0, _) => "No changes".to_owned(),
            (n, 0) => format!("{} changed", plural(n, "file")),
            (n, d) if d == n => format!("{} changed, all reviewed", plural(n, "file")),
            (n, d) => format!("{} changed, {} to review", plural(n, "file"), n - d),
        };
        if filtered > 0 {
            summary.push_str(&format!(" ({filtered} not compared)"));
        }
        ui.horizontal(|ui| {
            let label = ui.label(RichText::new(summary).size(small).color(weak));
            if filtered > 0 {
                label.on_hover_text(format!("Not compared: {} that Git converts with a filter (such as Git LFS). Use Git to see their changes.", plural(filtered, "file")));
            }
            if done > 0 {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("Clear").on_hover_text("Untick every file and forget the changes you kept").clicked() {
                        action = Some(ListAction::ClearReviewed);
                    }
                });
            }
        });
        ui.separator();
        // Rows: files to review, then (collapsed) the reviewed ones.
        let (todo, reviewed): (Vec<&FileChange>, Vec<&FileChange>) = files.iter().partition(|f| !self.review.is_done(f));
        let mut rows: Vec<Option<&FileChange>> = todo.into_iter().map(Some).collect();
        if !reviewed.is_empty() {
            rows.push(None);
            if self.review.show_reviewed {
                rows.extend(reviewed.iter().map(|f| Some(*f)));
            }
        }
        let current = self.docs.get(self.active).and_then(|d| d.compare.as_ref()?.review.clone());
        let light = self.docs[self.active].highlighter.is_light();
        let row_h = ui.spacing().interact_size.y.max(small + 8.0);
        let n_reviewed = reviewed.len();
        egui::ScrollArea::vertical().auto_shrink(false).show_rows(ui, row_h, rows.len(), |ui, range| {
            for row in &rows[range] {
                let Some(f) = row else {
                    let arrow = if self.review.show_reviewed { "⏷" } else { "⏵" };
                    if ui.add(egui::Button::new(RichText::new(format!("{arrow} Reviewed ({n_reviewed})")).size(small).color(weak)).frame(false)).clicked() {
                        self.review.show_reviewed = !self.review.show_reviewed;
                    }
                    continue;
                };
                ui.horizontal(|ui| {
                    ui.set_height(row_h);
                    let mut ticked = self.review.is_done(f);
                    let tick = ui.add_enabled(!f.kept, egui::Checkbox::without_text(&mut ticked));
                    if tick.on_hover_text(if f.kept { "Every change kept" } else { "Reviewed" }).changed() {
                        action = Some(ListAction::Reviewed(f.path.clone(), ticked));
                    }
                    ui.label(RichText::new(f.status.letter()).monospace().strong().color(status_color(f.status, light))).on_hover_text(f.status.label());
                    let (dir, name) = match f.rel.rsplit_once('/') {
                        Some((d, n)) => (format!("{d}/"), n.to_owned()),
                        None => (String::new(), f.rel.clone()),
                    };
                    crate::fonts::ensure_cjk(ui.ctx(), &f.rel);
                    let open = self.docs.iter().find(|d| d.path.as_ref() == Some(&f.path));
                    let mark = match open {
                        Some(d) if d.conflict => "⚠ ",
                        Some(d) if d.is_dirty() => "● ",
                        _ => "",
                    };
                    let mut job = egui::text::LayoutJob::default();
                    let body = egui::TextStyle::Body.resolve(ui.style());
                    let fg = ui.visuals().text_color();
                    job.append(&format!("{mark}{name}"), 0.0, egui::TextFormat::simple(body.clone(), if self.review.is_done(f) { weak } else { fg }));
                    if !dir.is_empty() {
                        job.append(&format!("  {dir}"), 0.0, egui::TextFormat::simple(egui::FontId::proportional(small), weak));
                    }
                    job.wrap.max_rows = 1;
                    job.wrap.break_anywhere = true;
                    job.wrap.max_width = (ui.available_width() - 80.0).max(40.0);
                    let selected = current.as_ref() == Some(&f.path);
                    let mut hover = match &f.old {
                        Some((_, old)) => format!("{} (renamed from {old})", f.rel),
                        None => f.rel.clone(),
                    };
                    if let Some(why) = &f.blocked {
                        hover.push('\n');
                        hover.push_str(why);
                    }
                    if let Some(d) = open {
                        if d.conflict {
                            hover.push_str("\nChanged on disk while its tab has unsaved edits: the list shows the file on disk, the review tab your edits");
                        } else if d.is_dirty() {
                            hover.push_str("\nUnsaved edits in Strata: the list shows the file on disk");
                        }
                    }
                    let resp = ui.selectable_label(selected, job).on_hover_text(hover);
                    if resp.clicked() {
                        action = Some(ListAction::Open(f.path.clone()));
                    }
                    resp.context_menu(|ui| {
                        if ui.button("Show Changes").clicked() {
                            action = Some(ListAction::Open(f.path.clone()));
                            ui.close();
                        }
                        let ticked = self.review.reviewed.contains_key(&f.path);
                        if ui.button(if ticked { "Mark as Not Reviewed" } else { "Mark as Reviewed" }).clicked() {
                            action = Some(ListAction::Reviewed(f.path.clone(), !ticked));
                            ui.close();
                        }
                        ui.separator();
                        if ui.add_enabled(!f.binary && f.blocked.is_none(), egui::Button::new("Keep File")).clicked() {
                            action = Some(ListAction::KeepFile(f.path.clone()));
                            ui.close();
                        }
                        if ui.add_enabled(!f.binary && f.blocked.is_none() && !f.alias, egui::Button::new("Revert File…")).on_hover_text("Put the whole file back as it was").clicked() {
                            action = Some(ListAction::RevertFile(f.path.clone()));
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("Copy Path").clicked() {
                            ui.ctx().copy_text(f.path.display().to_string());
                            ui.close();
                        }
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let (add, del) = match (f.lines, f.binary) {
                            _ if f.blocked.is_some() => (None, Some("can't review".to_owned())),
                            (_, true) => (None, Some("binary".to_owned())),
                            (Some((0, 0)), _) if f.status == Status::Modified => (None, Some("line endings".to_owned())),
                            (Some((a, r)), _) => ((a > 0).then(|| format!("+{a}")), (r > 0).then(|| format!("−{r}"))),
                            _ if f.alias => (None, None),
                            (None, _) => (None, Some("large".to_owned())),
                        };
                        // Right to left: removed, then added.
                        if let Some(d) = del {
                            let color = if !d.starts_with('−') { weak } else { status_color(Status::Deleted, light) };
                            ui.label(RichText::new(d).size(small).monospace().color(color));
                        }
                        if let Some(a) = add {
                            ui.label(RichText::new(a).size(small).monospace().color(status_color(Status::Added, light)));
                        }
                    });
                });
            }
        });
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::tests::{run, temp_repo};
    use std::time::{Duration, Instant};

    fn text(app: &StrataApp, i: usize) -> String {
        let b = &app.docs[i].editor.buffer;
        b.slice(0..b.len_chars())
    }

    /// Ask for a scan and wait for it.
    fn rescan(app: &mut StrataApp) {
        app.review_request(None);
        let deadline = Instant::now() + Duration::from_secs(30);
        while app.review.waiting || app.review.starting {
            app.review_poll();
            assert!(Instant::now() < deadline, "no scan result");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Wait for a review tab waiting on its base to open, and for the Trash.
    fn settle(app: &mut StrataApp) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while app.review.pending_open.is_some() || app.review.pending_trash.is_some() {
            app.review_poll();
            assert!(Instant::now() < deadline, "the base never came");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Open `path`'s review tab (its base is read in the background).
    fn open(app: &mut StrataApp, path: &Path) -> bool {
        app.open_review(path);
        settle(app);
        app.docs.get(app.active).and_then(|d| d.compare.as_ref()?.review.as_deref()) == Some(path)
    }

    fn listed(app: &StrataApp) -> Vec<(String, Status)> {
        app.review.scan.as_ref().unwrap().files.iter().map(|f| (f.rel.clone(), f.status)).collect()
    }

    fn app_for(dir: &Path) -> StrataApp {
        let mut app = StrataApp::new(Some(dir.to_owned()), None);
        app.review_show();
        rescan(&mut app);
        app
    }

    /// The flow end to end against HEAD: an "agent" edits files outside
    /// Strata; the list shows them; one change is reverted, one kept, the
    /// revert is undone and redone, and what's saved is byte-exact.
    #[test]
    fn review_against_head_keep_revert_undo_save() {
        let Some(dir) = temp_repo("ui-head") else { return };
        let mut app = app_for(&dir);
        assert_eq!(listed(&app), vec![]);
        let main = dir.join("src/main.rs");
        let crlf = dir.join("crlf.txt");
        std::fs::write(&main, "fn main() {\n    zero();\n    one();\n    TWO();\n}\n").unwrap();
        std::fs::write(&crlf, "a\r\nB\r\nc\r\nd\r\n").unwrap();
        std::fs::write(dir.join("new.txt"), "brand new\n").unwrap();
        std::fs::remove_file(dir.join("gone.txt")).unwrap();
        rescan(&mut app);
        assert_eq!(
            listed(&app),
            vec![("crlf.txt".into(), Status::Modified), ("gone.txt".into(), Status::Deleted), ("new.txt".into(), Status::Added), ("src/main.rs".into(), Status::Modified)]
        );
        assert_eq!(app.review_remaining(), Some(4));

        // main.rs: two changes. Revert the first, keep the second.
        assert!(open(&mut app, &main));
        let t = app.active;
        assert_eq!(app.docs[t].compare.as_ref().unwrap().hunks().len(), 2);
        assert_eq!(app.docs.iter().filter(|d| d.path.as_ref() == Some(&main)).count(), 1, "only the review tab");
        app.review_revert_change(t);
        assert_eq!(text(&app, t), "fn main() {\n    one();\n    TWO();\n}\n");
        assert!(app.docs[t].is_dirty());
        // Undo brings the agent's line back; redo reverts again.
        app.docs[t].editor.undo();
        assert_eq!(text(&app, t), "fn main() {\n    zero();\n    one();\n    TWO();\n}\n");
        app.docs[t].editor.redo();
        assert_eq!(text(&app, t), "fn main() {\n    one();\n    TWO();\n}\n");
        app.review_step_change(true);
        settle(&mut app);
        app.review_keep_change(t);
        assert_eq!(app.docs[t].compare.as_ref().unwrap().hunks().len(), 0);
        assert!(app.status.starts_with("Kept. No changes left"), "{}", app.status);
        assert!(app.save(t, false));
        assert_eq!(std::fs::read(&main).unwrap(), b"fn main() {\n    one();\n    TWO();\n}\n");
        rescan(&mut app);
        let f = app.review.file(&main).unwrap().clone();
        assert!(f.kept, "saved as kept: done");
        assert_eq!(app.review_remaining(), Some(3));

        // crlf.txt: revert the whole file, save: byte-exact CRLF.
        assert!(open(&mut app, &crlf));
        let t = app.active;
        assert!(app.docs.iter().all(|d| d.compare.as_ref().is_none_or(|c| c.review.as_ref() == Some(&crlf))), "the earlier review tab closed");
        app.review_revert_file(t);
        assert!(app.save(t, false));
        assert_eq!(std::fs::read(&crlf).unwrap(), b"a\r\nb\r\nc\r\n");

        // gone.txt: Revert File puts the text back in a tab; saving restores it.
        let gone = dir.join("gone.txt");
        assert!(open(&mut app, &gone));
        let t = app.active;
        app.review_revert_file(t);
        assert!(app.save(t, false));
        assert_eq!(std::fs::read(&gone).unwrap(), b"bye\n");

        // new.txt: Revert File asks, then moves it to the Trash.
        let new = dir.join("new.txt");
        assert!(open(&mut app, &new));
        let t = app.active;
        app.review_revert_file(t);
        assert!(app.review.confirm.is_some());
        app.review_confirmed();
        settle(&mut app);
        assert!(!new.exists());

        rescan(&mut app);
        assert_eq!(listed(&app), vec![("src/main.rs".into(), Status::Modified)]);
        // Git agrees: only main.rs differs, by exactly the kept change.
        let diff = crate::git::git(&dir, &["diff", "--no-ext-diff", "--no-color", "HEAD", "--", "."]).unwrap();
        let diff = String::from_utf8(diff).unwrap();
        assert!(diff.contains("-    two();\n+    TWO();\n"), "{diff}");
        assert!(!diff.contains("zero"), "{diff}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Files in legacy encodings, UTF-16 and LF-only files come back byte
    /// for byte; a base that isn't clean text is refused, not corrupted.
    #[test]
    fn reverted_files_come_back_byte_exact() {
        let Some(dir) = temp_repo("ui-bytes") else { return };
        let sjis = encoding_rs::SHIFT_JIS.encode("日本語のテキストファイルです。\nこれは文字コードの判定のためのテストです。\n").0.into_owned();
        let w1252 = encoding_rs::WINDOWS_1252.encode("Café, naïve, déjà vu, crème brûlée, façade, señor, über, ½ price, “quotes”.\n").0.into_owned();
        let wide: Vec<u8> = [0xFF, 0xFE].into_iter().chain("one\r\ntwo\r\n".encode_utf16().flat_map(u16::to_le_bytes)).collect();
        std::fs::write(dir.join("sjis.txt"), &sjis).unwrap();
        std::fs::write(dir.join("w1252.txt"), &w1252).unwrap();
        std::fs::write(dir.join("wide.txt"), &wide).unwrap();
        std::fs::write(dir.join("lf.txt"), "one\ntwo\n").unwrap();
        std::fs::write(dir.join("broken.txt"), b"\xEF\xBB\xBFok \xFF\xFE\xFD\n").unwrap();
        run(&dir, &["add", "."]);
        run(&dir, &["commit", "-q", "-m", "encodings"]);
        let mut app = app_for(&dir);
        // The agent deletes one, rewrites one as UTF-8, edits UTF-16, turns LF into CRLF.
        std::fs::remove_file(dir.join("sjis.txt")).unwrap();
        std::fs::write(dir.join("w1252.txt"), "Café, naïve, déjà vu.\nNew line.\n").unwrap();
        let edited: Vec<u8> = [0xFF, 0xFE].into_iter().chain("one\r\nTWO\r\n".encode_utf16().flat_map(u16::to_le_bytes)).collect();
        std::fs::write(dir.join("wide.txt"), edited).unwrap();
        std::fs::write(dir.join("lf.txt"), "one\r\ntwo\r\n").unwrap();
        std::fs::write(dir.join("broken.txt"), b"changed\n").unwrap();
        rescan(&mut app);
        for (name, original) in [("sjis.txt", sjis.clone()), ("w1252.txt", w1252.clone()), ("wide.txt", wide.clone()), ("lf.txt", b"one\ntwo\n".to_vec())] {
            let path = dir.join(name);
            assert!(open(&mut app, &path), "{name}: {}", app.status);
            let t = app.active;
            app.review_revert_file(t);
            assert!(app.save(t, false), "{name}");
            assert!(std::fs::read(&path).unwrap() == original, "{name} isn't byte for byte what was committed");
        }
        assert!(!open(&mut app, &dir.join("broken.txt")));
        assert!(app.status.contains("isn't clean text"), "{}", app.status);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// README.md renamed to Readme.md is one file (on a case-insensitive
    /// disk, the same file): listed as a rename, never trashed by Revert.
    #[test]
    fn a_respelled_file_is_never_trashed() {
        let Some(dir) = temp_repo("ui-case") else { return };
        // café.txt, its name in composed form (NFC), as most tools write it.
        std::fs::write(dir.join("caf\u{e9}.txt"), "accent\n").unwrap();
        run(&dir, &["add", "."]);
        run(&dir, &["commit", "-q", "-m", "accent"]);
        let mut app = app_for(&dir);
        // Renamed by letter case, and by Unicode form (decomposed, NFD).
        let renames = [("README.md", "Readme.md", "# Title\n\nText.\n"), ("caf\u{e9}.txt", "cafe\u{301}.txt", "accent\n")];
        for (old, new, _) in renames {
            std::fs::rename(dir.join(old), dir.join("tmp")).unwrap();
            std::fs::rename(dir.join("tmp"), dir.join(new)).unwrap();
        }
        rescan(&mut app);
        for (old, new, text) in renames {
            let path = dir.join(new);
            let f = app.review.file(&path).unwrap_or_else(|| panic!("{new} not listed: {:?}", listed(&app))).clone();
            assert_eq!(f.status, Status::Renamed, "{new}");
            // Whether the disk treats both names as one file decides how it's shown.
            assert_eq!(f.alias, review::same_file(&dir.join(old), &path), "{new}");
            assert!(open(&mut app, &path));
            let t = app.active;
            app.review_revert_file(t);
            if f.alias {
                assert!(app.review.confirm.is_none(), "no Trash question for {new}");
                assert!(app.status.contains("respelled"), "{}", app.status);
                // Even if asked somehow, nothing moves.
                app.review.confirm = Some(Confirm::of(&app.review.file(&path).unwrap().clone(), None));
                app.review_confirmed();
                settle(&mut app);
            }
            assert!(path.exists() || dir.join(old).exists(), "{new}: the only copy went");
            if f.alias {
                assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
            }
            app.review.confirm = None;
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Between the question and the answer, the agent writes new work into
    /// the file (or brings a renamed file's old name back): nothing moves.
    #[test]
    fn nothing_is_trashed_that_changed_after_the_question() {
        let Some(dir) = temp_repo("ui-toctou") else { return };
        let mut app = app_for(&dir);
        let new = dir.join("new.txt");
        std::fs::write(&new, "first draft\n").unwrap();
        std::fs::rename(dir.join("moved.txt"), dir.join("moved2.txt")).unwrap();
        rescan(&mut app);
        assert!(open(&mut app, &new));
        let t = app.active;
        app.review_revert_file(t);
        assert!(app.review.confirm.is_some());
        // The agent's new work lands while the question is up.
        std::fs::write(&new, "the agent's second draft, longer\n").unwrap();
        app.review_confirmed();
        settle(&mut app);
        assert!(app.status.contains("changed since it was listed"), "{}", app.status);
        assert_eq!(std::fs::read_to_string(&new).unwrap(), "the agent's second draft, longer\n");
        // Same size, same second: the bytes still decide.
        rescan(&mut app);
        assert!(open(&mut app, &new));
        let t = app.active;
        app.review_revert_file(t);
        std::fs::write(&new, "the agent's second draft, LONGER\n").unwrap();
        app.review_confirmed();
        settle(&mut app);
        assert!(new.exists(), "a same-size rewrite was trashed");

        // A rename whose old name comes back meanwhile.
        let moved = dir.join("moved2.txt");
        assert!(open(&mut app, &moved));
        let t = app.active;
        app.review_revert_file(t);
        assert!(app.review.confirm.is_some());
        std::fs::write(dir.join("moved.txt"), "someone else's new file\n").unwrap();
        app.review_confirmed();
        settle(&mut app);
        assert!(moved.exists());
        assert_eq!(std::fs::read_to_string(dir.join("moved.txt")).unwrap(), "someone else's new file\n");
        assert!(app.status.contains("exists again"), "{}", app.status);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Trashing happens off the UI thread, one file at a time.
    #[test]
    fn the_trash_runs_in_the_background_once() {
        let Some(dir) = temp_repo("ui-bgtrash") else { return };
        let mut app = app_for(&dir);
        let new = dir.join("new.txt");
        std::fs::write(&new, "new\n").unwrap();
        rescan(&mut app);
        assert!(open(&mut app, &new));
        let t = app.active;
        app.review_revert_file(t);
        app.review_confirmed();
        assert!(app.review.pending_trash.is_some(), "the Trash is the worker's job");
        assert!(app.status.starts_with("Moving new.txt"), "{}", app.status);
        // Asking again while it's on its way does nothing.
        app.review_revert_file(t);
        app.review_confirmed();
        settle(&mut app);
        assert!(!new.exists());
        assert!(app.status.starts_with("Moved new.txt"), "{}", app.status);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The last line of defense: a "new" file that is the same file as a
    /// tracked one (here a hard link, on any disk) is never trashed.
    #[cfg(unix)]
    #[test]
    fn a_new_file_that_is_a_tracked_one_is_never_trashed() {
        let Some(dir) = temp_repo("ui-twin") else { return };
        let mut app = app_for(&dir);
        let twin = dir.join("twin.md");
        std::fs::hard_link(dir.join("README.md"), &twin).unwrap();
        std::fs::write(dir.join("README.md"), "# Title\n\nText, edited through the twin.\n").unwrap();
        rescan(&mut app);
        assert_eq!(app.review.file(&twin).map(|f| f.status), Some(Status::Added));
        assert!(open(&mut app, &twin));
        let t = app.active;
        app.review_revert_file(t);
        assert!(app.review.confirm.is_none(), "no Trash question");
        assert!(app.status.contains("same file as README.md"), "{}", app.status);
        app.review.confirm = Some(Confirm::of(&app.review.file(&twin).unwrap().clone(), None));
        app.review_confirmed();
        settle(&mut app);
        assert!(twin.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A committed file or folder replaced by a link to somewhere outside
    /// the repository: listed, but never opened, reverted or saved through.
    #[cfg(unix)]
    #[test]
    fn links_out_of_the_repository_are_never_followed() {
        let Some(dir) = temp_repo("ui-links") else { return };
        let outside = std::env::temp_dir().join(format!("strata-review-outside-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&outside);
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), "do not touch\n").unwrap();
        std::fs::write(outside.join("main.rs"), "outside main\n").unwrap();
        let mut app = app_for(&dir);
        std::fs::remove_file(dir.join("gone.txt")).unwrap();
        std::os::unix::fs::symlink(outside.join("secret.txt"), dir.join("gone.txt")).unwrap();
        std::fs::remove_dir_all(dir.join("src")).unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("src")).unwrap();
        rescan(&mut app);
        let files = app.review.scan.as_ref().unwrap().files.clone();
        for name in ["gone.txt", "src/main.rs"] {
            let f = files.iter().find(|f| f.rel == name).unwrap_or_else(|| panic!("{name} not listed: {files:?}"));
            assert!(f.blocked.is_some(), "{name}");
            assert!(!open(&mut app, &f.path), "{name} opened");
        }
        assert_eq!(std::fs::read_to_string(outside.join("secret.txt")).unwrap(), "do not touch\n");
        assert_eq!(std::fs::read_to_string(outside.join("main.rs")).unwrap(), "outside main\n");
        assert!(app.docs.iter().all(|d| d.path.as_ref().is_none_or(|p| !p.starts_with(&dir))), "no tab of a linked file");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&outside);
    }

    /// A tick covers what you saw and saved; the agent writing right after
    /// your save brings the file back.
    #[test]
    fn a_tick_covers_only_what_you_saved() {
        let Some(dir) = temp_repo("ui-tick") else { return };
        let mut app = app_for(&dir);
        let readme = dir.join("README.md");
        std::fs::write(&readme, "# Title\n\nAgent.\n").unwrap();
        rescan(&mut app);
        assert!(open(&mut app, &readme));
        app.review_set_reviewed(&readme, true);
        let t = app.active;
        app.docs[t].editor.insert_text("// mine\n");
        assert!(app.save(t, false));
        // Before any scan sees my save, the agent writes again.
        std::fs::write(&readme, "# Title\n\nAgent again.\n").unwrap();
        rescan(&mut app);
        assert!(!app.review.reviewed.contains_key(&readme), "the agent's new text isn't covered by my tick");
        // Without the agent's write, my own save keeps the tick.
        app.review_set_reviewed(&readme, true);
        app.docs[t].check_disk();
        app.docs[t].editor.insert_text("x");
        assert!(app.save(t, false));
        rescan(&mut app);
        assert!(app.review.reviewed.contains_key(&readme));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A big file is listed without line counts but still reviews and
    /// reverts exactly; a binary one is listed and refused politely.
    #[test]
    fn large_and_binary_files() {
        let Some(dir) = temp_repo("ui-large") else { return };
        let big: String = (0..250_000).map(|i| format!("line {i:06} of a large generated file\n")).collect();
        assert!(big.len() > 8 << 20);
        std::fs::write(dir.join("big.txt"), &big).unwrap();
        std::fs::write(dir.join("blob.bin"), [0u8, 1, 2, 3, 0, 255]).unwrap();
        run(&dir, &["add", "."]);
        run(&dir, &["commit", "-q", "-m", "big"]);
        let mut app = app_for(&dir);
        let edited = big.replacen("line 100000 of", "LINE 100000 of", 1);
        std::fs::write(dir.join("big.txt"), &edited).unwrap();
        std::fs::write(dir.join("blob.bin"), [0u8, 9, 9]).unwrap();
        rescan(&mut app);
        let files = app.review.scan.as_ref().unwrap().files.clone();
        assert_eq!(files.len(), 2);
        assert_eq!((files[0].rel.as_str(), files[0].lines, files[0].binary), ("big.txt", None, false));
        assert_eq!((files[1].rel.as_str(), files[1].binary), ("blob.bin", true));
        assert!(!app.open_review(&dir.join("blob.bin")));
        assert!(app.status.contains("isn't text"), "{}", app.status);
        assert!(open(&mut app, &dir.join("big.txt")));
        let t = app.active;
        assert_eq!(app.docs[t].compare.as_ref().unwrap().hunks().len(), 1);
        app.review_revert_change(t);
        assert!(app.save(t, false));
        assert!(std::fs::read(dir.join("big.txt")).unwrap() == big.as_bytes());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Whole frames and real key presses: the list updates by itself while
    /// files change on disk, and F8 / ⌘⇧F8 / ⌘Z / ⌘S review, revert, undo
    /// and save.
    #[test]
    fn keyboard_review_in_whole_frames() {
        use super::super::ui_tests::{frame, press};
        use egui::{Key, Modifiers};
        let Some(dir) = temp_repo("ui-keys") else { return };
        let primary = if cfg!(target_os = "macos") { Modifiers::MAC_CMD | Modifiers::COMMAND } else { Modifiers::CTRL | Modifiers::COMMAND };
        let ctx = egui::Context::default();
        let mut app = StrataApp::new(Some(dir.clone()), None);
        app.run(&ctx, crate::commands::Cmd::ReviewShowChanges);
        let wait = |app: &mut StrataApp, done: &dyn Fn(&StrataApp) -> bool| {
            let deadline = Instant::now() + Duration::from_secs(30);
            while !done(app) {
                frame(app, &ctx, vec![]);
                assert!(Instant::now() < deadline, "timed out: {:?}", app.review.scan.as_ref().map(|s| s.files.len()));
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        wait(&mut app, &|a| a.review.scan.is_some());
        assert_eq!(listed(&app), vec![]);
        // The agent edits a file; the list notices without being asked.
        let main = dir.join("src/main.rs");
        std::fs::write(&main, "fn main() {\n    ONE();\n    two();\n    three();\n}\n").unwrap();
        wait(&mut app, &|a| a.review.scan.as_ref().is_some_and(|s| s.files.len() == 1));
        assert_eq!(app.review_remaining(), Some(1));

        frame(&mut app, &ctx, vec![press(Key::F8, Modifiers::NONE)]);
        // The tab opens once the earlier version has been read, frames later.
        wait(&mut app, &|a| a.docs[a.active].compare.as_ref().is_some_and(|c| c.review.is_some()));
        let t = app.active;
        assert_eq!(app.docs[t].compare.as_ref().and_then(|c| c.review.clone()), Some(main.clone()));
        assert_eq!(app.docs[t].editor.cursor_line_col().0, 2, "on the first change");
        frame(&mut app, &ctx, vec![press(Key::F8, primary | Modifiers::SHIFT)]);
        assert_eq!(text(&app, app.active), "fn main() {\n    one();\n    two();\n    three();\n}\n");
        frame(&mut app, &ctx, vec![press(Key::Z, primary)]);
        assert_eq!(text(&app, app.active), "fn main() {\n    ONE();\n    two();\n    three();\n}\n", "undo brings the change back");
        frame(&mut app, &ctx, vec![press(Key::F8, primary | Modifiers::SHIFT)]);
        // F8 again: on to the second change (three();), which is kept.
        frame(&mut app, &ctx, vec![press(Key::F8, Modifiers::NONE)]);
        frame(&mut app, &ctx, vec![press(Key::F8, primary)]);
        assert_eq!(app.docs[app.active].compare.as_ref().unwrap().hunks().len(), 0);
        frame(&mut app, &ctx, vec![press(Key::S, primary)]);
        assert_eq!(std::fs::read(&main).unwrap(), b"fn main() {\n    one();\n    two();\n    three();\n}\n");
        wait(&mut app, &|a| a.review_remaining() == Some(0));
        // The agent changes the file again: the open review tab reloads and
        // shows the new change, and the file is back on the list.
        std::fs::write(&main, "// header\nfn main() {\n    one();\n    two();\n    three();\n}\n").unwrap();
        wait(&mut app, &|a| a.review_remaining() == Some(1));
        wait(&mut app, &|a| a.docs[a.active].compare.as_ref().is_some_and(|c| c.hunks().len() == 1));
        assert!(text(&app, app.active).starts_with("// header\n"));
        // With unsaved edits, another change on disk doesn't overwrite them:
        // the tab is marked changed on disk, the list still shows the file.
        let t = app.active;
        app.docs[t].editor.insert_text("x");
        std::fs::write(&main, "// header 2\nfn main() {\n    one();\n    two();\n    three();\n}\n").unwrap();
        wait(&mut app, &|a| a.docs[t].conflict);
        assert!(text(&app, t).contains("x"), "my edit is kept");
        assert_eq!(app.review_remaining(), Some(1));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Start Review: only what changes afterwards is listed; ticks and
    /// stepping across files; End Review goes back to HEAD.
    #[test]
    fn review_from_a_snapshot() {
        let Some(dir) = temp_repo("ui-snap") else { return };
        // My own uncommitted edit, before the review.
        std::fs::write(dir.join("README.md"), "# Title\n\nMine.\n").unwrap();
        let mut app = app_for(&dir);
        assert_eq!(listed(&app), vec![("README.md".into(), Status::Modified)]);
        // A review tab open from before follows the new base.
        assert!(open(&mut app, &dir.join("README.md")));
        let t = app.active;
        assert_eq!(app.docs[t].compare.as_ref().unwrap().base_text(), "# Title\n\nText.\n");
        app.review_start(None);
        rescan(&mut app);
        assert!(app.review.reviewing());
        rescan(&mut app);
        assert_eq!(listed(&app), vec![]);
        assert_eq!(app.docs[t].compare.as_ref().unwrap().base_text(), "# Title\n\nMine.\n");
        assert!(app.close_tab(t));

        // The agent works: edits my file, edits another, adds one, renames one.
        std::fs::write(dir.join("README.md"), "# Title\n\nMine.\nAgent.\n").unwrap();
        std::fs::write(dir.join("src/main.rs"), "fn main() {\n    one();\n}\n").unwrap();
        std::fs::rename(dir.join("moved.txt"), dir.join("src/moved.txt")).unwrap();
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "agent"]);
        rescan(&mut app);
        assert_eq!(listed(&app), vec![("README.md".into(), Status::Modified), ("src/main.rs".into(), Status::Modified), ("src/moved.txt".into(), Status::Renamed)]);

        // Next Change walks into the first file; its base is the snapshot, not HEAD.
        app.review_step_change(true);
        settle(&mut app);
        let t = app.active;
        assert_eq!(app.docs[t].compare.as_ref().unwrap().base_text(), "# Title\n\nMine.\n");
        assert_eq!(app.docs[t].compare.as_ref().unwrap().review.as_deref(), Some(dir.join("README.md").as_path()));
        // Past its last change, Next Change moves on to the next file.
        app.review_step_change(true);
        settle(&mut app);
        let t = app.active;
        assert_eq!(app.docs[t].compare.as_ref().unwrap().review.as_deref(), Some(dir.join("src/main.rs").as_path()));
        assert_eq!(app.docs[t].compare.as_ref().unwrap().base_text(), "fn main() {\n    one();\n    two();\n}\n");
        // Mark as reviewed: ticked, and on to the next file.
        app.review_mark_current();
        settle(&mut app);
        assert!(app.review.reviewed.contains_key(&dir.join("src/main.rs")));
        assert_eq!(app.review_remaining(), Some(2));
        let t = app.active;
        assert_eq!(app.docs[t].compare.as_ref().unwrap().review.as_deref(), Some(dir.join("src/moved.txt").as_path()));
        // A ticked file that changes again comes back.
        rescan(&mut app);
        std::fs::write(dir.join("src/main.rs"), "fn main() {\n}\n").unwrap();
        rescan(&mut app);
        assert!(!app.review.reviewed.contains_key(&dir.join("src/main.rs")));

        app.review_end();
        rescan(&mut app);
        assert!(!app.review.reviewing());
        // The agent committed README.md and the rename; main.rs changed since.
        assert_eq!(listed(&app), vec![("src/main.rs".into(), Status::Modified)]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
