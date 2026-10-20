//! What each helper sends: the instructions and the material, built off the
//! UI thread (reading earlier versions runs git), within the size limit.
//!
//! Only what the helper is about goes in: the files Review Changes lists
//! (which never includes ignored files), minus binary files and the ones it
//! can't compare (links, Git filters); the change at the caret; the
//! selection. File names of what's left out aren't sent either.

use super::Prompt;
use crate::diff::Hunk;
use crate::encoding::Encoding;
use crate::review::{self, Base, Scan, Status};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

const SYSTEM: &str = "You are a careful senior software engineer helping someone in their code editor. Answer in concise Markdown. Work only from the material provided; when something can't be told from it, say so instead of guessing.";

/// Below this much room, a file is left out rather than cut to a stub.
const MIN_PIECE: usize = 512;
/// Files bigger than this aren't read (Review Changes doesn't diff them either).
const MAX_FILE: u64 = 64 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ask {
    Summarize,
    Risks,
    CommitMessage,
}

pub enum Task {
    /// Every changed file: the review's (against its base), or against the
    /// last commit for a commit message. `scan` is the list as Review
    /// Changes last had it, when it's for the same base; `open` holds the
    /// text of open tabs by path (unsaved edits count).
    Changes { ask: Ask, folder: PathBuf, base: Base, scan: Option<Scan>, open: HashMap<PathBuf, String> },
    /// One file in a review tab: its base and its text now.
    File { ask: Ask, rel: String, base: String, current: String },
    /// The change at the caret in a review tab.
    Hunk { rel: String, base: String, current: String, hunk: Hunk },
    /// The editor's selection.
    Selection { name: String, syntax: String, text: String, path: Option<PathBuf> },
}

/// A prompt ready to send, with what the user should know about it.
#[derive(Clone, Debug)]
pub struct Prepared {
    pub title: String,
    pub prompt: Prompt,
    /// Truncation and files left out, for the consent question and the panel.
    pub notes: Vec<String>,
}

fn instructions(ask: Ask, what: &str) -> String {
    match ask {
        Ask::Summarize => format!("Summarize {what}. Start with a short overview paragraph, then a bulleted list of the notable changes per file. Mention anything that looks unfinished or accidental."),
        Ask::Risks => format!(
            "Review {what} for risks: bugs, security problems, data loss, breaking changes, missing error handling, performance problems and missing tests. For each risk give the file and lines, why it matters and how serious it is (high, medium or low), most serious first. If you find nothing significant, say so plainly; don't describe harmless changes."
        ),
        Ask::CommitMessage => format!(
            "Write a Git commit message for {what}: a summary line of at most 72 characters in the imperative mood, a blank line, then a short body saying what changed and why, wrapped at 72 columns. Reply with the commit message alone: no code fences, no preamble."
        ),
    }
}

fn kb(bytes: usize) -> usize {
    bytes.div_ceil(1024)
}

/// A fence longer than any run of backticks in `body`.
fn fence(body: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for c in body.chars() {
        run = if c == '`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    "`".repeat((longest + 1).max(3))
}

/// The longest prefix of `s` within `max` bytes that ends at a line break
/// (or a character boundary, for one long line).
fn cut(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    match s[..end].rfind('\n') {
        Some(i) if i > 0 => &s[..i + 1],
        _ => &s[..end],
    }
}

/// Fenced pieces added while they fit in `limit` bytes.
struct Builder {
    out: String,
    limit: usize,
    added: usize,
    cut_short: bool,
    full: bool,
    left_out: usize,
}

impl Builder {
    fn new(intro: String, limit: usize) -> Self {
        Builder { out: intro, limit, added: 0, cut_short: false, full: false, left_out: 0 }
    }

    /// Add `header` and `body` in a fence; cut the body short if it's the
    /// one that reaches the limit, and leave out everything after that.
    fn add(&mut self, header: &str, body: &str, lang: &str) {
        if self.full {
            self.left_out += 1;
            return;
        }
        let f = fence(body);
        let marker = "… (cut short here: over the size limit)\n";
        let overhead = header.len() + 2 * f.len() + lang.len() + 8;
        let room = self.limit.saturating_sub(self.out.len() + overhead);
        let piece = if body.len() <= room {
            body.to_owned()
        } else if room >= MIN_PIECE + marker.len() {
            self.cut_short = true;
            self.full = true;
            format!("{}{marker}", cut(body, room - marker.len()))
        } else {
            self.full = true;
            self.left_out += 1;
            return;
        };
        let nl = if piece.ends_with('\n') { "" } else { "\n" };
        self.out.push_str(&format!("{header}\n{f}{lang}\n{piece}{nl}{f}\n\n"));
        self.added += 1;
    }
}

/// Text as compared: decoded like Review Changes does, line breaks as `\n`.
/// None for binary or undecodable bytes.
fn text_of(bytes: &[u8]) -> Option<String> {
    if crate::binary::classify(&bytes[..bytes.len().min(8192)]) != crate::binary::Kind::Text {
        return None;
    }
    let (text, _) = review::decode_base(bytes, Encoding::UTF8).ok()?;
    Some(text.replace("\r\n", "\n"))
}

/// Build the prompt for `task`, at most `limit` bytes of material.
pub fn prepare(task: Task, limit: usize) -> Result<Prepared, String> {
    match task {
        Task::Changes { ask, folder, base, scan, open } => changes(ask, &folder, base, scan, &open, limit),
        Task::File { ask, rel, base, current } => {
            let base = base.replace("\r\n", "\n");
            let current = current.replace("\r\n", "\n");
            let diff = crate::git::unified_diff(&base, &current, &format!("a/{rel}"), &format!("b/{rel}")).ok_or_else(|| format!("{rel} has no changes left to send"))?;
            let mut b = Builder::new(format!("{}\n\n", instructions(ask, &format!("the changes to {rel}"))), limit);
            b.add(&format!("### {rel}"), &diff, "diff");
            if b.added == 0 {
                return Err(format!("The changes to {rel} don't fit in the {} KB limit (ai_max_prompt_kb)", kb(limit)));
            }
            let notes = if b.cut_short { vec![format!("The diff is over the {} KB limit: only its start is sent.", kb(limit))] } else { Vec::new() };
            let title = match ask {
                Ask::Risks => format!("Flag Risks: {rel}"),
                _ => format!("Summarize: {rel}"),
            };
            Ok(Prepared { title, prompt: Prompt { system: SYSTEM.into(), user: b.out.trim_end().to_owned() }, notes })
        }
        Task::Hunk { rel, base, current, hunk } => {
            let body = hunk_text(&base.replace("\r\n", "\n"), &current.replace("\r\n", "\n"), &hunk);
            let mut b = Builder::new(format!("Explain this change to {rel}: what it does and why someone might have made it. Point out anything that looks wrong. Lines starting with - were removed, + added, the rest are unchanged context.\n\n"), limit);
            b.add(&format!("### {rel}"), &body, "diff");
            if b.added == 0 {
                return Err(format!("The change doesn't fit in the {} KB limit (ai_max_prompt_kb)", kb(limit)));
            }
            let notes = if b.cut_short { vec![format!("The change is over the {} KB limit: only its start is sent.", kb(limit))] } else { Vec::new() };
            Ok(Prepared { title: format!("Explain Change: {rel}"), prompt: Prompt { system: SYSTEM.into(), user: b.out.trim_end().to_owned() }, notes })
        }
        Task::Selection { name, syntax, text, path } => {
            if text.trim().is_empty() {
                return Err("Select some text first".into());
            }
            if let Some(p) = &path
                && ignored(p)
            {
                return Err(format!("{name} is ignored by Git, so Strata won't send any of it (the AI helpers never send ignored files)"));
            }
            let lang = syntax.to_lowercase().split_whitespace().next().unwrap_or("").to_owned();
            let mut b = Builder::new(format!("Explain this code from {name} ({syntax}): what it does, how it works, and anything surprising or risky in it.\n\n"), limit);
            b.add(&format!("### {name}"), &text.replace("\r\n", "\n"), &lang);
            if b.added == 0 {
                return Err(format!("The selection doesn't fit in the {} KB limit (ai_max_prompt_kb)", kb(limit)));
            }
            let notes = if b.cut_short { vec![format!("The selection is over the {} KB limit: only its start is sent.", kb(limit))] } else { Vec::new() };
            Ok(Prepared { title: format!("Explain Selection: {name}"), prompt: Prompt { system: SYSTEM.into(), user: b.out.trim_end().to_owned() }, notes })
        }
    }
}

/// Git ignores `path` (`git check-ignore`, which only reads ignore files).
fn ignored(path: &Path) -> bool {
    let (Some(dir), Some(p)) = (path.parent(), path.to_str()) else { return false };
    if !dir.is_dir() {
        return false;
    }
    crate::git::git_io(dir, &["check-ignore", "--", p], None, 1 << 16, Duration::from_secs(10)).is_some_and(|out| !out.trim_ascii().is_empty())
}

/// One change with a few unchanged lines around it, in diff style.
fn hunk_text(base: &str, current: &str, h: &Hunk) -> String {
    const CONTEXT: usize = 6;
    let a: Vec<&str> = base.split('\n').collect();
    let b: Vec<&str> = current.split('\n').collect();
    let get = |v: &[&str], r: std::ops::Range<usize>| -> Vec<String> { v[r.start.min(v.len())..r.end.min(v.len())].iter().map(|s| (*s).to_owned()).collect() };
    let before = get(&b, h.b.start.saturating_sub(CONTEXT)..h.b.start);
    let after = get(&b, h.b.end..h.b.end + CONTEXT);
    let mut out = format!("@@ -{},{} +{},{} @@\n", h.a.start + 1, h.a.len(), h.b.start + 1, h.b.len());
    for l in before {
        out.push_str(&format!(" {l}\n"));
    }
    for l in get(&a, h.a.clone()) {
        out.push_str(&format!("-{l}\n"));
    }
    for l in get(&b, h.b.clone()) {
        out.push_str(&format!("+{l}\n"));
    }
    for l in after {
        out.push_str(&format!(" {l}\n"));
    }
    out
}

fn changes(ask: Ask, folder: &Path, base: Base, scan: Option<Scan>, open: &HashMap<PathBuf, String>, limit: usize) -> Result<Prepared, String> {
    let scan = match scan {
        Some(s) => s,
        None => review::scan_once(folder, &base)?,
    };
    let against = match (&base, ask) {
        (_, Ask::CommitMessage) | (Base::Head, _) => "the last commit",
        (Base::Snapshot(_), _) => "the start of the review",
    };
    let mut skipped_binary = 0;
    let mut skipped_other = 0;
    let mut pieces: Vec<(String, String)> = Vec::new();
    for f in &scan.files {
        if f.blocked.is_some() || f.filtered || !review::inside(&scan.top, &f.path) || f.old.as_ref().is_some_and(|(p, _)| !review::inside(&scan.top, p)) {
            skipped_other += 1;
            continue;
        }
        if f.binary {
            skipped_binary += 1;
            continue;
        }
        let base_rel = f.old.as_ref().map_or(&f.rel, |o| &o.1);
        let old_text = if f.status == Status::Added {
            String::new()
        } else {
            match review::base_bytes(&scan.top, &base, scan.commit.as_deref(), base_rel) {
                Some(bytes) => match text_of(&bytes) {
                    Some(t) => t,
                    None => {
                        skipped_binary += 1;
                        continue;
                    }
                },
                None => {
                    skipped_other += 1;
                    continue;
                }
            }
        };
        let new_text = if f.status == Status::Deleted {
            String::new()
        } else if let Some(t) = open.get(&f.path) {
            t.replace("\r\n", "\n")
        } else {
            let too_big = std::fs::metadata(&f.path).map_or(true, |m| m.len() > MAX_FILE);
            match (!too_big).then(|| std::fs::read(&f.path).ok()).flatten().and_then(|b| text_of(&b)) {
                Some(t) => t,
                None => {
                    skipped_binary += 1;
                    continue;
                }
            }
        };
        let Some(diff) = crate::git::unified_diff(&old_text, &new_text, &format!("a/{base_rel}"), &format!("b/{}", f.rel)) else { continue };
        let what = match (&f.old, f.status) {
            (Some((_, old)), _) => format!("renamed from {old}"),
            (None, s) => s.label().to_lowercase(),
        };
        pieces.push((format!("### {} ({what})", f.rel), diff));
    }
    if pieces.is_empty() {
        return Err(if scan.files.is_empty() { format!("Nothing has changed since {against}") } else { "None of the changed files can be sent (they're binary, or Strata can't compare them)".to_owned() });
    }
    let total = pieces.len();
    let subject = match ask {
        Ask::CommitMessage => format!("these changes ({total} file{})", if total == 1 { "" } else { "s" }),
        _ => format!("the changes in this code review ({total} file{}, compared with {against})", if total == 1 { "" } else { "s" }),
    };
    let mut b = Builder::new(format!("{}\n\n", instructions(ask, &subject)), limit);
    for (header, diff) in &pieces {
        b.add(header, diff, "diff");
    }
    if b.left_out > 0 {
        b.out.push_str(&format!("({} more changed file{} left out to keep within the size limit.)\n", b.left_out, if b.left_out == 1 { " was" } else { "s were" }));
    }
    if b.added == 0 {
        return Err(format!("The first file's changes don't fit in the {} KB limit (ai_max_prompt_kb)", kb(limit)));
    }
    let mut notes = Vec::new();
    if b.left_out > 0 || b.cut_short {
        notes.push(format!("Over the {} KB limit (ai_max_prompt_kb): {} of {total} files sent{}.", kb(limit), b.added, if b.cut_short { ", the last one cut short" } else { "" }));
    }
    let skipped = skipped_binary + skipped_other;
    if skipped > 0 {
        notes.push(format!("Not sent: {skipped} file{} that {} binary or that Strata can't compare (links, Git filters).", if skipped == 1 { "" } else { "s" }, if skipped == 1 { "is" } else { "are" }));
    }
    let title = match ask {
        Ask::Summarize => "Summarize Changes",
        Ask::Risks => "Flag Risks",
        Ask::CommitMessage => "Draft Commit Message",
    };
    Ok(Prepared { title: format!("{title} ({total} file{})", if total == 1 { "" } else { "s" }), prompt: Prompt { system: SYSTEM.into(), user: b.out.trim_end().to_owned() }, notes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::tests::{run, temp_repo};

    #[test]
    fn fences_outgrow_backticks_and_cuts_end_at_lines() {
        assert_eq!(fence("plain"), "```");
        assert_eq!(fence("a ```` b"), "`````");
        assert_eq!(cut("one\ntwo\nthree\n", 9), "one\ntwo\n");
        assert_eq!(cut("ééé", 3), "é");
    }

    #[test]
    fn review_changes_respect_the_limit_and_skip_binary_and_ignored() {
        let Some(dir) = temp_repo("ai-prompt") else { return };
        std::fs::write(dir.join("src/main.rs"), "fn main() {\n    one();\n    three();\n}\n").unwrap();
        std::fs::write(dir.join("src/new.rs"), "pub fn added() {}\n").unwrap();
        std::fs::write(dir.join("image.bin"), b"\x00\x01\x02binary\x00").unwrap();
        std::fs::create_dir_all(dir.join("target")).unwrap();
        std::fs::write(dir.join("target/secret.txt"), "IGNORED-SECRET\n").unwrap();
        std::fs::write(dir.join(".env"), "TOKEN=hunter2\n").unwrap();
        run(&dir, &["add", "-f", ".env"]);
        run(&dir, &["commit", "-q", "-m", "env"]);
        std::fs::write(dir.join(".gitignore"), "target/\n.env\n").unwrap();
        std::fs::write(dir.join("README.md"), "# Title\n\nSaved text.\n").unwrap();
        let mut open = HashMap::new();
        // A changed file open with unsaved edits: its text in the tab is what's sent.
        open.insert(dir.join("README.md"), "# Title\n\nUnsaved text.\n".to_owned());
        let p = prepare(Task::Changes { ask: Ask::Summarize, folder: dir.clone(), base: Base::Head, scan: None, open: open.clone() }, 200 << 10).unwrap();
        let u = &p.prompt.user;
        assert!(u.contains("+    three();") && u.contains("+pub fn added() {}") && u.contains("+Unsaved text."), "{u}");
        assert!(!u.contains("IGNORED-SECRET") && !u.contains("binary") && !u.contains("image.bin"), "{u}");
        assert!(p.notes.iter().any(|n| n.contains("Not sent: 1 file")), "{:?}", p.notes);
        // A tight limit sends fewer files and says so.
        let small = prepare(Task::Changes { ask: Ask::Risks, folder: dir.clone(), base: Base::Head, scan: None, open }, 800).unwrap();
        assert!(small.prompt.user.len() <= 800 + 200, "{}", small.prompt.user.len());
        assert!(small.notes.iter().any(|n| n.contains("Over the 1 KB limit")), "{:?}", small.notes);
        assert!(small.prompt.user.contains("left out to keep within the size limit"));
        // Explain Selection refuses an ignored file, even one Git tracked once.
        std::fs::write(dir.join("target/x.rs"), "fn x() {}\n").unwrap();
        let sel = |path: PathBuf| Task::Selection { name: "x".into(), syntax: "Rust".into(), text: "fn x() {}".into(), path: Some(path) };
        assert!(prepare(sel(dir.join("target/x.rs")), 1 << 20).unwrap_err().contains("ignored"));
        assert!(prepare(sel(dir.join("src/main.rs")), 1 << 20).is_ok());
        // Nothing changed: said plainly.
        let clean = temp_repo("ai-prompt-clean").unwrap();
        assert!(prepare(Task::Changes { ask: Ask::CommitMessage, folder: clean.clone(), base: Base::Head, scan: None, open: HashMap::new() }, 1 << 20).unwrap_err().contains("Nothing has changed"));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&clean);
    }

    #[test]
    fn hunks_and_selections() {
        let base = "a\nb\nc\nd\ne\n";
        let cur = "a\nb\nC\nd\ne\n";
        let p = prepare(Task::Hunk { rel: "x.txt".into(), base: base.into(), current: cur.into(), hunk: Hunk { a: 2..3, b: 2..3 } }, 1 << 20).unwrap();
        assert!(p.prompt.user.contains("-c\n+C\n") && p.prompt.user.contains(" b\n"), "{}", p.prompt.user);
        let long = "x".repeat(5000) + "\n";
        let s = prepare(Task::Selection { name: "a.rs".into(), syntax: "Rust".into(), text: long.repeat(3), path: None }, 8 << 10).unwrap();
        assert!(s.prompt.user.len() < 9 << 10 && s.notes[0].contains("8 KB"), "{:?}", s.notes);
        assert!(prepare(Task::Selection { name: "a".into(), syntax: "Plain Text".into(), text: "  ".into(), path: None }, 1 << 20).is_err());
    }
}
