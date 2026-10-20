//! What each helper sends: the instructions and the material, built off the
//! UI thread (reading earlier versions runs git), within the size limit.
//!
//! Only what the helper is about goes in: the files Review Changes lists,
//! minus files that look like secrets (`SECRET_NAMES`), files Git's ignore
//! rules match (tracked ones too), binary files and the ones it can't
//! compare (links, Git filters); the change at the caret; the selection.
//! Each file is read from the repository's top without following links and
//! only if it's still exactly what the scan listed (`read_listed`), since a
//! listed path can be swapped for a link between the scan and the read.
//! Names of what's left out aren't sent.

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
    /// One file in a review tab (`path`): its base and its text now.
    File { ask: Ask, path: PathBuf, rel: String, base: String, current: String },
    /// The change at the caret in a review tab.
    Hunk { path: PathBuf, rel: String, base: String, current: String, hunk: Hunk },
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
        Task::File { ask, path, rel, base, current } => {
            check_file(&path, &rel)?;
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
        Task::Hunk { path, rel, base, current, hunk } => {
            check_file(&path, &rel)?;
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
            if text.contains('\0') {
                return Err(format!("The selection in {name} isn't text, so Strata won't send it"));
            }
            match &path {
                Some(p) => check_file(p, &name)?,
                None if looks_secret(&name) => return Err(format!("{name} looks like a secret, so Strata won't send any of it")),
                None => {}
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

/// File names never sent, whatever Git says about them: they usually hold
/// keys and passwords. Shown in the manual's AI Helpers page.
pub const SECRET_NAMES: &[&str] = &[".env", ".env.*", "*.pem", "*.key", "id_rsa*", "id_ed25519*", "*.p12", "*.pfx", ".npmrc", ".pypirc", "credentials*", "*.keystore"];

/// The file name (last part of `rel`) matches `SECRET_NAMES`, ignoring case.
pub fn looks_secret(rel: &str) -> bool {
    let name = rel.rsplit(['/', '\\']).next().unwrap_or(rel).to_lowercase();
    SECRET_NAMES.iter().any(|p| match (p.strip_prefix('*'), p.strip_suffix('*')) {
        (Some(ext), _) => name.ends_with(ext),
        (_, Some(start)) => name.starts_with(start),
        _ => name == *p,
    })
}

/// Which of `rels` Git's ignore rules match, tracked files included
/// (`git check-ignore --no-index`, which only reads ignore files). Err when
/// Git can't say: then nothing is sent.
fn ignored_set(top: &Path, rels: &[&str]) -> Result<std::collections::HashSet<String>, String> {
    let rels: Vec<&str> = rels.iter().copied().filter(|r| !r.contains('\0')).collect();
    if rels.is_empty() {
        return Ok(Default::default());
    }
    let input: Vec<u8> = rels.iter().flat_map(|r| [r.as_bytes(), b"\0"]).flatten().copied().collect();
    let out = crate::git::git_io_code(top, &["check-ignore", "--no-index", "--stdin", "-z"], Some(input), 16 << 20, Duration::from_secs(60));
    match out {
        Some((Some(0), out)) => Ok(out.split(|&b| b == 0).filter(|r| !r.is_empty()).map(|r| String::from_utf8_lossy(r).into_owned()).collect()),
        Some((Some(1), _)) => Ok(Default::default()),
        _ => Err("Couldn't check which files Git ignores, so nothing was sent".into()),
    }
}

/// Whether text from `path` may be sent (Explain Selection, a review tab):
/// not a secret's name, not a link or reached through one, not binary on
/// disk, and in a repository, not ignored (tracked or not) and not
/// converted by a Git filter. Err says why not.
fn check_file(path: &Path, name: &str) -> Result<(), String> {
    if looks_secret(name) || path.file_name().is_some_and(|n| looks_secret(&n.to_string_lossy())) {
        return Err(format!("{name} looks like a secret, so Strata won't send any of it (see the list in the manual)"));
    }
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(format!("{name} is a link, so Strata won't send any of it"));
    }
    // The repository as the path spells it (a `.git` in a folder above), so
    // a linked folder on the way is caught; else as Git finds it.
    let lexical = path.ancestors().skip(1).find(|a| std::fs::symlink_metadata(a.join(".git")).is_ok()).map(Path::to_path_buf);
    let file = match lexical.or_else(|| review::repo_top(path.parent().unwrap_or(Path::new(".")))) {
        Some(top) => {
            let opened = crate::claim::open_inside(&top, path).map_err(|_| format!("{name} is reached through a link, or isn't a plain file, so Strata won't send any of it"))?;
            let rel = path.strip_prefix(&top).ok().map(|r| r.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")).ok_or_else(|| format!("{name} isn't inside its repository"))?;
            if ignored_set(&top, &[&rel])?.contains(&rel) {
                return Err(format!("{name} is ignored by Git, so Strata won't send any of it (the AI helpers never send ignored files)"));
            }
            if let Some(f) = review::filter_of(&top, &rel) {
                return Err(format!("Git converts {name} with a filter ({f}), so Strata won't send any of it"));
            }
            opened.file
        }
        None => crate::claim::open_plain(path).map_err(|_| format!("{name} isn't a plain file on disk, so Strata won't send any of it"))?,
    };
    if !file.metadata().is_ok_and(|m| m.is_file()) {
        return Err(format!("{name} isn't a plain file, so Strata won't send any of it"));
    }
    let mut head = Vec::new();
    let _ = std::io::Read::read_to_end(&mut std::io::Read::take(&file, 8192), &mut head);
    if crate::binary::classify(&head) == crate::binary::Kind::Binary {
        return Err(format!("{name} isn't text, so Strata won't send any of it"));
    }
    Ok(())
}

/// Why a listed file wasn't read.
enum Unread {
    /// No longer the file the list showed (swapped, a link now, edited since).
    Changed,
    TooBig,
}

/// A listed file's bytes, read from the repository's top without following
/// any link, and only if it's still exactly the file the scan listed.
fn read_listed(top: &Path, f: &review::FileChange) -> Result<Vec<u8>, Unread> {
    let expected = review::expected(f).ok_or(Unread::Changed)?;
    if expected.len > MAX_FILE {
        return Err(Unread::TooBig);
    }
    #[cfg(test)]
    BEFORE_READ.with(|h| {
        if let Some(h) = h.borrow_mut().as_mut() {
            h(&f.path)
        }
    });
    let opened = crate::claim::open_inside(top, &f.path).map_err(|_| Unread::Changed)?;
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut std::io::Read::take(&opened.file, MAX_FILE + 1), &mut bytes).map_err(|_| Unread::Changed)?;
    if crate::claim::is_listed(&opened.file, &bytes, &expected) { Ok(bytes) } else { Err(Unread::Changed) }
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

#[cfg(test)]
type ReadHook = Box<dyn FnMut(&Path)>;

#[cfg(test)]
thread_local! {
    /// Tests: runs just before a listed file is read (to swap it for a link then).
    static BEFORE_READ: std::cell::RefCell<Option<ReadHook>> = const { std::cell::RefCell::new(None) };
}

fn changes(ask: Ask, folder: &Path, base: Base, scan: Option<Scan>, open: &HashMap<PathBuf, String>, limit: usize) -> Result<Prepared, String> {
    let given = scan.is_some();
    let scan = match scan {
        Some(s) => s,
        None => review::scan_once(folder, &base)?,
    };
    let mut got = collect(&scan, &base, open)?;
    // The list Review Changes had is a few seconds old: a file edited since
    // doesn't match it. Look again rather than leave it out.
    let scan = if given && got.changed > 0 {
        let fresh = review::scan_once(folder, &base)?;
        got = collect(&fresh, &base, open)?;
        fresh
    } else {
        scan
    };
    let against = match (&base, ask) {
        (_, Ask::CommitMessage) | (Base::Head, _) => "the last commit",
        (Base::Snapshot(_), _) => "the start of the review",
    };
    let mut notes = Vec::new();
    if !got.secrets.is_empty() {
        notes.push(format!("Left out (looks like a secret): {}.", got.secrets.join(", ")));
    }
    if got.ignored > 0 {
        notes.push(format!("Left out: {} file{} Git ignores.", got.ignored, if got.ignored == 1 { "" } else { "s" }));
    }
    let skipped = got.binary + got.other;
    if skipped > 0 {
        notes.push(format!("Not sent: {skipped} file{} that {} binary, too big, or that Strata can't compare (links, Git filters).", if skipped == 1 { "" } else { "s" }, if skipped == 1 { "is" } else { "are" }));
    }
    if got.changed > 0 {
        notes.push(format!("Not sent: {} file{} that changed while Strata was reading {}. Try again.", got.changed, if got.changed == 1 { "" } else { "s" }, if got.changed == 1 { "it" } else { "them" }));
    }
    let pieces = got.pieces;
    if pieces.is_empty() {
        return Err(if scan.files.is_empty() {
            format!("Nothing has changed since {against}")
        } else {
            format!("None of the changed files can be sent. {}", notes.join(" "))
        });
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
    if b.left_out > 0 || b.cut_short {
        notes.insert(0, format!("Over the {} KB limit (ai_max_prompt_kb): {} of {total} files sent{}.", kb(limit), b.added, if b.cut_short { ", the last one cut short" } else { "" }));
    }
    let title = match ask {
        Ask::Summarize => "Summarize Changes",
        Ask::Risks => "Flag Risks",
        Ask::CommitMessage => "Draft Commit Message",
    };
    Ok(Prepared { title: format!("{title} ({total} file{})", if total == 1 { "" } else { "s" }), prompt: Prompt { system: SYSTEM.into(), user: b.out.trim_end().to_owned() }, notes })
}

/// The diffs that can be sent from one scan, and what was left out.
#[derive(Default)]
struct Collected {
    pieces: Vec<(String, String)>,
    /// Relative paths left out by name (`SECRET_NAMES`), for the user only.
    secrets: Vec<String>,
    ignored: usize,
    binary: usize,
    other: usize,
    changed: usize,
}

fn collect(scan: &Scan, base: &Base, open: &HashMap<PathBuf, String>) -> Result<Collected, String> {
    let mut got = Collected::default();
    let mut candidates = Vec::new();
    for f in &scan.files {
        let names = [Some(f.rel.as_str()), f.old.as_ref().map(|o| o.1.as_str())];
        if names.iter().flatten().any(|n| looks_secret(n)) {
            got.secrets.push(f.rel.clone());
        } else if f.blocked.is_some() || f.filtered || !review::inside(&scan.top, &f.path) || f.old.as_ref().is_some_and(|(p, _)| !review::inside(&scan.top, p)) {
            got.other += 1;
        } else if f.binary {
            got.binary += 1;
        } else {
            candidates.push(f);
        }
    }
    // Ignore rules apply to tracked files too (a tracked .env added to .gitignore later).
    let rels: Vec<&str> = candidates.iter().flat_map(|f| [Some(f.rel.as_str()), f.old.as_ref().map(|o| o.1.as_str())]).flatten().collect();
    let ignored = ignored_set(&scan.top, &rels)?;
    for f in candidates {
        let base_rel = f.old.as_ref().map_or(&f.rel, |o| &o.1);
        if ignored.contains(&f.rel) || ignored.contains(base_rel) {
            got.ignored += 1;
            continue;
        }
        let old_text = if f.status == Status::Added {
            String::new()
        } else {
            match review::base_bytes(&scan.top, base, scan.commit.as_deref(), base_rel) {
                Some(bytes) => match text_of(&bytes) {
                    Some(t) => t,
                    None => {
                        got.binary += 1;
                        continue;
                    }
                },
                None => {
                    got.other += 1;
                    continue;
                }
            }
        };
        let new_text = if f.status == Status::Deleted {
            String::new()
        } else {
            // Checked on disk even when a tab has unsaved edits: the file must
            // still be the one listed, inside the repository.
            let bytes = match read_listed(&scan.top, f) {
                Ok(b) => b,
                Err(Unread::Changed) => {
                    got.changed += 1;
                    continue;
                }
                Err(Unread::TooBig) => {
                    got.other += 1;
                    continue;
                }
            };
            match open.get(&f.path) {
                Some(t) => t.replace("\r\n", "\n"),
                None => match text_of(&bytes) {
                    Some(t) => t,
                    None => {
                        got.binary += 1;
                        continue;
                    }
                },
            }
        };
        let Some(diff) = crate::git::unified_diff(&old_text, &new_text, &format!("a/{base_rel}"), &format!("b/{}", f.rel)) else { continue };
        let what = match (&f.old, f.status) {
            (Some((_, old)), _) => format!("renamed from {old}"),
            (None, s) => s.label().to_lowercase(),
        };
        got.pieces.push((format!("### {} ({what})", f.rel), diff));
    }
    Ok(got)
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

    /// A secret outside the repository, for links to point at.
    fn outside_secret(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("strata-ai-outside-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("id_rsa"), "-----BEGIN OUTSIDE SECRET-----\n").unwrap();
        std::fs::write(dir.join("main.rs"), "OUTSIDE-SECRET-IN-A-FOLDER\n").unwrap();
        dir
    }

    /// Security review, HIGH 1: a listed file swapped for a link (to the
    /// file, or through a folder) after the scan is never read or sent.
    #[cfg(unix)]
    #[test]
    fn files_swapped_for_links_after_the_scan_are_never_read() {
        let Some(dir) = temp_repo("ai-swap") else { return };
        let outside = outside_secret("swap");
        std::fs::write(dir.join("src/main.rs"), "fn main() {\n    one();\n    three();\n}\n").unwrap();
        std::fs::write(dir.join("README.md"), "# Title\n\nChanged.\n").unwrap();
        let scan = review::scan_once(&dir, &Base::Head).unwrap();
        assert_eq!(scan.files.len(), 2);
        // Swapped at the last moment, just before each file is read (after
        // any checks on its path): the file itself becomes a link, and a
        // folder on the way becomes a link to a folder with the same name in it.
        let (top, out) = (dir.clone(), outside.clone());
        BEFORE_READ.with(|h| {
            *h.borrow_mut() = Some(Box::new(move |p: &Path| {
                if p.ends_with("README.md") && !p.is_symlink() {
                    std::fs::remove_file(p).unwrap();
                    std::os::unix::fs::symlink(out.join("id_rsa"), p).unwrap();
                }
                if p.ends_with("src/main.rs") && !top.join("src").is_symlink() {
                    std::fs::remove_dir_all(top.join("src")).unwrap();
                    std::os::unix::fs::symlink(&out, top.join("src")).unwrap();
                }
            }))
        });
        let sent = prepare(Task::Changes { ask: Ask::Summarize, folder: dir.clone(), base: Base::Head, scan: Some(scan), open: HashMap::new() }, 1 << 20);
        BEFORE_READ.with(|h| *h.borrow_mut() = None);
        if let Ok(p) = &sent {
            assert!(!p.prompt.user.contains("OUTSIDE"), "{}", p.prompt.user);
        }
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&outside);
    }

    /// Security review, HIGH 2 and MEDIUM 3: Explain Selection refuses links,
    /// Git-filtered and binary files, ignored files (tracked ones too) and
    /// files that look like secrets.
    #[test]
    fn selections_from_files_that_must_not_be_sent() {
        let Some(dir) = temp_repo("ai-sel") else { return };
        let sel = |path: PathBuf| Task::Selection { name: path.file_name().unwrap().to_string_lossy().into_owned(), syntax: "Plain Text".into(), text: "some text".into(), path: Some(path) };
        // Filtered by Git.
        std::fs::write(dir.join(".gitattributes"), "*.dat filter=lfs\n").unwrap();
        std::fs::write(dir.join("big.dat"), "some text\n").unwrap();
        assert!(prepare(sel(dir.join("big.dat")), 1 << 20).is_err(), "filtered");
        // Binary on disk.
        std::fs::write(dir.join("blob.txt"), b"some text\x00\x01\x02").unwrap();
        assert!(prepare(sel(dir.join("blob.txt")), 1 << 20).is_err(), "binary");
        // Secrets, wherever they are.
        for name in [".env", ".env.local", "server.pem", "tls.key", "id_rsa", "id_ed25519.pub", "cert.p12", "cert.pfx", ".npmrc", ".pypirc", "credentials.json", "release.keystore"] {
            std::fs::write(dir.join(name), "some text\n").unwrap();
            assert!(prepare(sel(dir.join(name)), 1 << 20).unwrap_err().contains("secret"), "{name}");
        }
        // Tracked, then ignored.
        std::fs::write(dir.join("config.yml"), "some text\n").unwrap();
        run(&dir, &["add", "config.yml"]);
        run(&dir, &["commit", "-q", "-m", "config"]);
        std::fs::write(dir.join(".gitignore"), "target/\nconfig.yml\n").unwrap();
        assert!(prepare(sel(dir.join("config.yml")), 1 << 20).unwrap_err().contains("ignored"), "tracked but ignored");
        // A link.
        #[cfg(unix)]
        {
            let outside = outside_secret("sel");
            std::os::unix::fs::symlink(outside.join("main.rs"), dir.join("linked.rs")).unwrap();
            assert!(prepare(sel(dir.join("linked.rs")), 1 << 20).is_err(), "link to a file");
            std::os::unix::fs::symlink(&outside, dir.join("linkdir")).unwrap();
            assert!(prepare(sel(dir.join("linkdir/main.rs")), 1 << 20).is_err(), "through a linked folder");
            let _ = std::fs::remove_dir_all(&outside);
        }
        // An ordinary file is fine.
        assert!(prepare(sel(dir.join("src/main.rs")), 1 << 20).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Security review, MEDIUM 3: tracked files the ignore rules match, and
    /// files that look like secrets, are left out of review prompts, the
    /// secrets listed by name for the user only.
    #[test]
    fn review_prompts_leave_out_ignored_tracked_files_and_secrets() {
        let Some(dir) = temp_repo("ai-secrets") else { return };
        for (name, text) in [(".env", "TOKEN=old\n"), ("config.yml", "password: old\n"), ("deploy/server.pem", "OLD PEM\n")] {
            std::fs::create_dir_all(dir.join(name).parent().unwrap()).unwrap();
            std::fs::write(dir.join(name), text).unwrap();
            run(&dir, &["add", "-f", name]);
        }
        run(&dir, &["commit", "-q", "-m", "tracked"]);
        std::fs::write(dir.join(".gitignore"), "target/\n.env\nconfig.yml\n").unwrap();
        std::fs::write(dir.join(".env"), "TOKEN=NEW-SECRET-TOKEN\n").unwrap();
        std::fs::write(dir.join("config.yml"), "password: NEW-SECRET-PASSWORD\n").unwrap();
        std::fs::write(dir.join("deploy/server.pem"), "NEW-SECRET-PEM\n").unwrap();
        std::fs::write(dir.join("src/main.rs"), "fn main() {\n    one();\n    three();\n}\n").unwrap();
        let p = prepare(Task::Changes { ask: Ask::CommitMessage, folder: dir.clone(), base: Base::Head, scan: None, open: HashMap::new() }, 1 << 20).unwrap();
        let u = &p.prompt.user;
        assert!(u.contains("+    three();"), "{u}");
        // The .gitignore change itself (which names them) is sent; the files aren't.
        assert!(!u.contains("SECRET") && !u.contains("PEM") && !u.contains("password") && !u.contains("TOKEN"), "{u}");
        assert!(!u.contains("### .env") && !u.contains("### config.yml") && !u.contains("server.pem"), "{u}");
        assert!(p.notes.iter().any(|n| n.contains("looks like a secret") && n.contains(".env") && n.contains("deploy/server.pem")), "{:?}", p.notes);
        assert!(p.notes.iter().any(|n| n.contains("1 file Git ignores")), "{:?}", p.notes);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_manual_lists_every_secret_pattern() {
        let manual = include_str!("../../docs/manual/ai.md");
        for p in SECRET_NAMES {
            assert!(manual.contains(&format!("`{p}`")), "docs/manual/ai.md doesn't list {p}");
        }
    }

    #[test]
    fn hunks_and_selections() {
        let base = "a\nb\nc\nd\ne\n";
        let cur = "a\nb\nC\nd\ne\n";
        let dir = std::env::temp_dir().join(format!("strata-ai-hunk-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("x.txt"), cur).unwrap();
        let p = prepare(Task::Hunk { path: dir.join("x.txt"), rel: "x.txt".into(), base: base.into(), current: cur.into(), hunk: Hunk { a: 2..3, b: 2..3 } }, 1 << 20).unwrap();
        // A review tab of a file that looks like a secret sends nothing.
        std::fs::write(dir.join(".env"), cur).unwrap();
        assert!(prepare(Task::Hunk { path: dir.join(".env"), rel: ".env".into(), base: base.into(), current: cur.into(), hunk: Hunk { a: 2..3, b: 2..3 } }, 1 << 20).unwrap_err().contains("secret"));
        assert!(p.prompt.user.contains("-c\n+C\n") && p.prompt.user.contains(" b\n"), "{}", p.prompt.user);
        let long = "x".repeat(5000) + "\n";
        let s = prepare(Task::Selection { name: "a.rs".into(), syntax: "Rust".into(), text: long.repeat(3), path: None }, 8 << 10).unwrap();
        assert!(s.prompt.user.len() < 9 << 10 && s.notes[0].contains("8 KB"), "{:?}", s.notes);
        assert!(prepare(Task::Selection { name: "a".into(), syntax: "Plain Text".into(), text: "  ".into(), path: None }, 1 << 20).is_err());
    }
}
