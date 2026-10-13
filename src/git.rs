//! Git integration for the gutter and status bar: repo/branch lookup, the
//! file's text at HEAD (both via the `git` command line), a line diff against
//! it, and a background watcher so the UI never waits on git.

use std::collections::HashMap;
use std::io::Read;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime};

/// Kill a git process that runs longer than this.
const GIT_TIMEOUT: Duration = Duration::from_secs(5);
/// Don't diff files with more lines than this (either side).
const MAX_DIFF_LINES: usize = 200_000;
/// Give up on Myers past this edit distance; the rest is marked modified.
const MAX_EDIT_DISTANCE: usize = 1_000;
/// Skip HEAD blobs larger than this.
const MAX_HEAD_BYTES: usize = 32 * 1024 * 1024;
/// Re-read HEAD text / repo info at most this often (unless invalidated).
const REFRESH: Duration = Duration::from_secs(3);
/// Wait this long for typing to settle before diffing.
const DEBOUNCE: Duration = Duration::from_millis(40);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoInfo {
    /// Working tree root, as git reports it.
    pub root: PathBuf,
    /// Branch name, or the short commit hash when HEAD is detached.
    pub branch: String,
    pub detached: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineChange {
    Added,
    Modified,
    /// Lines removed just before this (empty) range's line; that line can be
    /// one past the last when the end of the file was removed.
    Deleted,
}

pub type Changes = Vec<(Range<usize>, LineChange)>;

// ---------------------------------------------------------------------------
// Running git
// ---------------------------------------------------------------------------

/// Folder to run git in for `path` (a file or a folder).
fn work_dir(path: &Path) -> Option<&Path> {
    if path.is_dir() { Some(path) } else { path.parent().filter(|p| p.is_dir()) }
}

/// Run git in `dir`; stdout on success. Never prompts, never hangs past the timeout.
fn git(dir: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let mut cmd = Command::new("git");
    cmd.args(["-c", "core.fsmonitor=false", "-c", "core.quotepath=false", "--no-pager"])
        .args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_ASKPASS", "")
        .env("SSH_ASKPASS", "")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    // Read on a helper thread so a full pipe can't deadlock the timeout loop.
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        stdout.read_to_end(&mut buf).map(|_| buf)
    });
    let deadline = Instant::now() + GIT_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(2)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    let out = reader.join().ok()?.ok()?;
    status.success().then_some(out)
}

fn git_line(dir: &Path, args: &[&str]) -> Option<String> {
    let out = git(dir, args)?;
    let s = String::from_utf8_lossy(&out).trim().to_owned();
    (!s.is_empty()).then_some(s)
}

/// Repo root and branch for a file or folder; None outside a repo or without git.
pub fn repo_info(path: &Path) -> Option<RepoInfo> {
    let dir = work_dir(path)?;
    let root = PathBuf::from(git_line(dir, &["rev-parse", "--show-toplevel"])?);
    if let Some(branch) = git_line(dir, &["branch", "--show-current"]) {
        return Some(RepoInfo { root, branch, detached: false });
    }
    let hash = git_line(dir, &["rev-parse", "--short", "HEAD"])?;
    Some(RepoInfo { root, branch: hash, detached: true })
}

/// The file's committed text at HEAD; None if untracked, new, binary-huge or not in a repo.
pub fn head_text(path: &Path) -> Option<String> {
    let dir = work_dir(path)?;
    let name = path.file_name()?.to_str()?;
    // `HEAD:./name` is resolved relative to the cwd, i.e. the repo-relative
    // path with forward slashes, without fighting symlinked/UNC root spellings.
    let out = git(dir, &["show", &format!("HEAD:./{name}")])?;
    if out.len() > MAX_HEAD_BYTES {
        return None;
    }
    Some(match String::from_utf8(out) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    })
}

// ---------------------------------------------------------------------------
// Line diff
// ---------------------------------------------------------------------------

/// Lines as the editor numbers them: split on '\n', a trailing '\r' ignored.
fn split_lines(s: &str) -> Vec<&str> {
    s.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect()
}

/// Changed line ranges in `current` relative to `base`, gutter style.
/// Empty when identical, or when either side exceeds the size limit.
pub fn diff_lines(base: &str, current: &str) -> Changes {
    let (a, b) = (split_lines(base), split_lines(current));
    if a.len() > MAX_DIFF_LINES || b.len() > MAX_DIFF_LINES {
        return Vec::new();
    }
    // Intern lines so the inner loop compares integers.
    let mut ids: HashMap<&str, u32> = HashMap::with_capacity(a.len() + b.len());
    fn intern<'a>(ids: &mut HashMap<&'a str, u32>, lines: &[&'a str]) -> Vec<u32> {
        lines
            .iter()
            .map(|&l| {
                let n = ids.len() as u32;
                *ids.entry(l).or_insert(n)
            })
            .collect()
    }
    let (a, b) = (intern(&mut ids, &a), intern(&mut ids, &b));

    let pre = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let suf = a[pre..].iter().rev().zip(b[pre..].iter().rev()).take_while(|(x, y)| x == y).count();
    let (am, bm) = (&a[pre..a.len() - suf], &b[pre..b.len() - suf]);

    let mut out = Vec::new();
    let mut push = |a_len: usize, b_start: usize, b_len: usize| {
        let kind = match (a_len, b_len) {
            (0, 0) => return,
            (_, 0) => LineChange::Deleted,
            (0, _) => LineChange::Added,
            _ => LineChange::Modified,
        };
        out.push((b_start..b_start + b_len, kind));
    };
    match myers(am, bm) {
        Some(matches) => {
            // Gaps between matched lines are hunks.
            let (mut i, mut j) = (0, 0);
            for (x, y) in matches.into_iter().chain(std::iter::once((am.len(), bm.len()))) {
                push(x - i, pre + j, y - j);
                (i, j) = (x + 1, y + 1);
            }
        }
        None => push(am.len(), pre, bm.len()),
    }
    out
}

/// Myers' O(ND) diff: matched (a, b) index pairs in order, or None past the edit limit.
fn myers(a: &[u32], b: &[u32]) -> Option<Vec<(usize, usize)>> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    if n == 0 || m == 0 {
        return Some(Vec::new());
    }
    let max = ((n + m) as usize).min(MAX_EDIT_DISTANCE) as isize;
    let off = max + 1;
    let mut v = vec![0isize; 2 * off as usize + 1];
    // trace[d][k + d] = furthest x on diagonal k after step d.
    let mut trace: Vec<Vec<u32>> = Vec::new();
    let mut done = false;
    for d in 0..=max {
        for k in (-d..=d).step_by(2) {
            let down = k == -d || (k != d && v[(off + k - 1) as usize] < v[(off + k + 1) as usize]);
            let mut x = if down { v[(off + k + 1) as usize] } else { v[(off + k - 1) as usize] + 1 };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[(off + k) as usize] = x;
            if x >= n && y >= m {
                done = true;
                break;
            }
        }
        trace.push(v[(off - d) as usize..=(off + d) as usize].iter().map(|&x| x as u32).collect());
        if done {
            break;
        }
    }
    if !done {
        return None;
    }
    // Walk back through the trace collecting each step's diagonal run.
    let get = |d: isize, k: isize| trace[d as usize][(k + d) as usize] as isize;
    let mut matches = Vec::new();
    let (mut x, mut y) = (n, m);
    for d in (0..trace.len() as isize).rev() {
        let k = x - y;
        let (px, py, sx) = if d == 0 {
            (0, 0, 0)
        } else {
            let down = k == -d || (k != d && get(d - 1, k - 1) < get(d - 1, k + 1));
            let pk = if down { k + 1 } else { k - 1 };
            let px = get(d - 1, pk);
            (px, px - pk, if down { px } else { px + 1 })
        };
        // The snake runs diagonally from x = sx up to x; collect it backwards.
        while x > sx {
            x -= 1;
            y -= 1;
            matches.push((x as usize, y as usize));
        }
        (x, y) = (px, py);
    }
    matches.reverse();
    Some(matches)
}

/// A unified diff (three lines of context) turning `old` into `new`, as
/// `diff -u` writes it under `---`/`+++` headers named `old_name`/`new_name`.
/// None when the texts are identical or too big to diff.
pub fn unified_diff(old: &str, new: &str, old_name: &str, new_name: &str) -> Option<String> {
    const CONTEXT: usize = 3;
    if old == new {
        return None;
    }
    fn lines(s: &str) -> Vec<&str> {
        let mut v = split_lines(s);
        // A final newline ends the last line rather than starting an empty one.
        if s.ends_with('\n') {
            v.pop();
        }
        if s.is_empty() {
            v.clear();
        }
        v
    }
    let (a, b) = (lines(old), lines(new));
    if a.len() > MAX_DIFF_LINES || b.len() > MAX_DIFF_LINES {
        return None;
    }
    let mut out = format!("--- {old_name}\n+++ {new_name}\n");
    if a == b {
        out.push_str("(Only line endings or the newline at the end of the file differ.)\n");
        return Some(out);
    }
    // Changed spans (old range, new range): the gaps between matched lines.
    let mut ids: HashMap<&str, u32> = HashMap::with_capacity(a.len() + b.len());
    fn intern<'a>(ids: &mut HashMap<&'a str, u32>, lines: &[&'a str]) -> Vec<u32> {
        lines
            .iter()
            .map(|&l| {
                let n = ids.len() as u32;
                *ids.entry(l).or_insert(n)
            })
            .collect()
    }
    let (ai, bi) = (intern(&mut ids, &a), intern(&mut ids, &b));
    let pre = ai.iter().zip(&bi).take_while(|(x, y)| x == y).count();
    let suf = ai[pre..].iter().rev().zip(bi[pre..].iter().rev()).take_while(|(x, y)| x == y).count();
    let (am, bm) = (&ai[pre..ai.len() - suf], &bi[pre..bi.len() - suf]);
    let matched = myers(am, bm).unwrap_or_default();
    let mut spans: Vec<(Range<usize>, Range<usize>)> = Vec::new();
    let (mut i, mut j) = (0, 0);
    for (x, y) in matched.into_iter().chain(std::iter::once((am.len(), bm.len()))) {
        if x > i || y > j {
            spans.push((pre + i..pre + x, pre + j..pre + y));
        }
        (i, j) = (x + 1, y + 1);
    }
    // Spans whose context would touch become one hunk.
    let mut k = 0;
    while k < spans.len() {
        let mut end = k;
        while end + 1 < spans.len() && spans[end + 1].0.start - spans[end].0.end <= 2 * CONTEXT {
            end += 1;
        }
        let a0 = spans[k].0.start.saturating_sub(CONTEXT);
        let a1 = (spans[end].0.end + CONTEXT).min(a.len());
        let b0 = spans[k].1.start - (spans[k].0.start - a0);
        let b1 = spans[end].1.end + (a1 - spans[end].0.end);
        let at = |start: usize, len: usize| if len == 0 { start } else { start + 1 };
        out.push_str(&format!("@@ -{},{} +{},{} @@\n", at(a0, a1 - a0), a1 - a0, at(b0, b1 - b0), b1 - b0));
        let mut pos = a0;
        for (ar, br) in &spans[k..=end] {
            for l in &a[pos..ar.start] {
                out.push_str(&format!(" {l}\n"));
            }
            for l in &a[ar.clone()] {
                out.push_str(&format!("-{l}\n"));
            }
            for l in &b[br.clone()] {
                out.push_str(&format!("+{l}\n"));
            }
            pos = ar.end;
        }
        for l in &a[pos..a1] {
            out.push_str(&format!(" {l}\n"));
        }
        k = end + 1;
    }
    Some(out)
}

/// Start line of the next (or previous) change after `line`, wrapping around.
pub fn next_change(changes: &[(Range<usize>, LineChange)], line: usize, forward: bool) -> Option<usize> {
    let starts = changes.iter().map(|(r, _)| r.start);
    if forward {
        let first = starts.clone().min()?;
        Some(starts.filter(|&s| s > line).min().unwrap_or(first))
    } else {
        // Inside a change, "previous" skips the one containing the cursor.
        let here = changes.iter().find(|(r, _)| r.contains(&line)).map(|(r, _)| r.start);
        let from = here.unwrap_or(line);
        let last = starts.clone().max()?;
        Some(starts.filter(|&s| s < from).max().unwrap_or(last))
    }
}

// ---------------------------------------------------------------------------
// Background watcher
// ---------------------------------------------------------------------------

struct Request {
    path: PathBuf,
    /// A rope snapshot is O(1) to take on the UI thread; the worker turns it into text.
    text: ropey::Rope,
    version: u64,
}

#[derive(Default)]
struct State {
    /// The newest request per file, oldest file first (split panes show
    /// several files, and each needs its own answer).
    pending: Vec<Request>,
    /// Paths whose cached HEAD text must be re-read (saved, checked out...).
    stale: Vec<PathBuf>,
    quit: bool,
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
    /// Repo info per folder, filled by the worker for the UI to read.
    repos: Mutex<HashMap<PathBuf, Option<RepoInfo>>>,
}

pub type DiffResult = (PathBuf, u64, Changes);

/// Diffs documents against HEAD on one worker thread. For each file only the
/// latest request matters: a newer one replaces that file's pending one.
pub struct GitWatcher {
    shared: Arc<Shared>,
    rx: Receiver<DiffResult>,
}

impl GitWatcher {
    /// `ctx` gets a repaint request whenever a result is ready.
    pub fn new(ctx: Option<egui::Context>) -> Self {
        let shared = Arc::new(Shared { state: Mutex::default(), wake: Condvar::new(), repos: Mutex::default() });
        let (tx, rx) = channel();
        let worker = shared.clone();
        std::thread::Builder::new()
            .name("git".into())
            .spawn(move || work(&worker, &tx, ctx.as_ref()))
            .expect("spawn git thread");
        Self { shared, rx }
    }

    /// Queue a diff of `text` (the document at `version`) against HEAD.
    pub fn request(&self, path: &Path, text: ropey::Rope, version: u64) {
        let mut st = self.shared.state.lock().unwrap();
        let req = Request { path: path.to_owned(), text, version };
        match st.pending.iter_mut().find(|r| r.path == path) {
            Some(r) => *r = req,
            None => st.pending.push(req),
        }
        self.shared.wake.notify_one();
    }

    /// Forget cached HEAD text and repo info for `path` (call after saving);
    /// takes effect with the next request.
    pub fn invalidate(&self, path: &Path) {
        self.shared.state.lock().unwrap().stale.push(path.to_owned());
    }

    /// The next finished diff, if any, oldest first. Call until None each frame.
    pub fn poll(&self) -> Option<DiffResult> {
        self.rx.try_recv().ok()
    }

    /// Cached repo info for a file's folder; None until a request for a file
    /// there has been processed, or outside a repo.
    pub fn repo(&self, path: &Path) -> Option<RepoInfo> {
        let dir = path.parent()?;
        self.shared.repos.lock().unwrap().get(dir).cloned().flatten()
    }
}

impl Drop for GitWatcher {
    fn drop(&mut self) {
        self.shared.state.lock().unwrap().quit = true;
        self.shared.wake.notify_one();
    }
}

struct HeadEntry {
    read: Instant,
    mtime: Option<SystemTime>,
    text: Option<Arc<String>>,
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn work(shared: &Shared, tx: &Sender<DiffResult>, ctx: Option<&egui::Context>) {
    let mut heads: HashMap<PathBuf, HeadEntry> = HashMap::new();
    let mut repo_read: HashMap<PathBuf, Instant> = HashMap::new();
    loop {
        let (reqs, stale) = {
            let mut st = shared.state.lock().unwrap();
            while st.pending.is_empty() && !st.quit {
                st = shared.wake.wait(st).unwrap();
            }
            // Debounce: let a burst of keystrokes settle into one request.
            while !st.quit {
                let (next, timeout) = shared.wake.wait_timeout(st, DEBOUNCE).unwrap();
                st = next;
                if timeout.timed_out() {
                    break;
                }
            }
            if st.quit {
                return;
            }
            (std::mem::take(&mut st.pending), std::mem::take(&mut st.stale))
        };
        for p in &stale {
            heads.remove(p);
            if let Some(dir) = p.parent() {
                repo_read.remove(dir);
            }
        }
        for req in reqs {
            if !diff_one(shared, req, &mut heads, &mut repo_read, tx, ctx) {
                return;
            }
        }
    }
}

/// Diff one request and send the result. False when the app has gone.
fn diff_one(
    shared: &Shared,
    req: Request,
    heads: &mut HashMap<PathBuf, HeadEntry>,
    repo_read: &mut HashMap<PathBuf, Instant>,
    tx: &Sender<DiffResult>,
    ctx: Option<&egui::Context>,
) -> bool {
    let Some(dir) = req.path.parent().map(Path::to_path_buf) else { return true };
    let now = Instant::now();
    if repo_read.get(&dir).is_none_or(|t| now.duration_since(*t) > REFRESH) {
        let info = repo_info(&req.path);
        shared.repos.lock().unwrap().insert(dir.clone(), info);
        repo_read.insert(dir.clone(), now);
    }
    let in_repo = shared.repos.lock().unwrap().get(&dir).is_some_and(Option::is_some);

    let changes = if in_repo {
        let disk = mtime(&req.path);
        let fresh = heads.get(&req.path).is_some_and(|h| h.mtime == disk && now.duration_since(h.read) <= REFRESH);
        if !fresh {
            let text = head_text(&req.path).map(Arc::new);
            heads.insert(req.path.clone(), HeadEntry { read: now, mtime: disk, text });
        }
        match &heads[&req.path].text {
            // Too long to diff: don't even copy it out of the rope.
            Some(_) if req.text.len_lines() > MAX_DIFF_LINES + 1 => Vec::new(),
            Some(base) => diff_lines(base, &String::from(&req.text)),
            None => Vec::new(), // untracked or new
        }
    } else {
        Vec::new()
    };
    if tx.send((req.path, req.version, changes)).is_err() {
        return false;
    }
    if let Some(ctx) = ctx {
        ctx.request_repaint();
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use LineChange::*;

    fn d(a: &str, b: &str) -> Changes {
        diff_lines(a, b)
    }

    #[test]
    fn unified_diff_hunks_and_context() {
        let old: String = (1..=20).map(|n| format!("line {n}\n")).collect();
        let new = old.replace("line 2\n", "line two\n").replace("line 18\n", "").replace("line 20\n", "line 20\nline 21\n");
        let diff = unified_diff(&old, &new, "a.txt (on disk)", "a.txt (unsaved)").unwrap();
        let expected = "--- a.txt (on disk)\n+++ a.txt (unsaved)\n\
@@ -1,5 +1,5 @@\n line 1\n-line 2\n+line two\n line 3\n line 4\n line 5\n\
@@ -15,6 +15,6 @@\n line 15\n line 16\n line 17\n-line 18\n line 19\n line 20\n+line 21\n";
        assert_eq!(diff, expected);
        assert_eq!(unified_diff("same\n", "same\n", "a", "b"), None);
        assert_eq!(unified_diff("", "new\n", "a", "b").unwrap(), "--- a\n+++ b\n@@ -0,0 +1,1 @@\n+new\n");
        assert!(unified_diff("x\n", "x", "a", "b").unwrap().contains("newline at the end"));
    }

    #[test]
    fn identical() {
        assert!(d("a\nb\nc\n", "a\nb\nc\n").is_empty());
        assert!(d("", "").is_empty());
    }

    #[test]
    fn pure_add() {
        assert_eq!(d("a\nb\n", "a\nx\ny\nb\n"), vec![(1..3, Added)]);
        assert_eq!(d("a\nb", "x\na\nb"), vec![(0..1, Added)]);
        assert_eq!(d("a\nb", "a\nb\nc"), vec![(2..3, Added)]);
    }

    #[test]
    fn pure_delete() {
        assert_eq!(d("x\na\nb\n", "a\nb\n"), vec![(0..0, Deleted)]);
        assert_eq!(d("a\nx\ny\nb\n", "a\nb\n"), vec![(1..1, Deleted)]);
        assert_eq!(d("a\nb\nc", "a\nb"), vec![(2..2, Deleted)]);
    }

    #[test]
    fn modification() {
        assert_eq!(d("a\nb\nc\n", "a\nB\nc\n"), vec![(1..2, Modified)]);
        assert_eq!(d("a\nb\nc\n", "a\nB\nB2\nc\n"), vec![(1..3, Modified)]);
    }

    #[test]
    fn mixed() {
        let base = "1\n2\n3\n4\n5\n6\n7\n8\n";
        let cur = "0\n1\n3\n4\nfive\n6\n7\n7.5\n8\n";
        assert_eq!(d(base, cur), vec![(0..1, Added), (2..2, Deleted), (4..5, Modified), (7..8, Added)]);
    }

    #[test]
    fn crlf_matches_lf() {
        assert!(d("a\nb\nc\n", "a\r\nb\r\nc\r\n").is_empty());
        assert_eq!(d("a\r\nb\r\n", "a\nB\n"), vec![(1..2, Modified)]);
    }

    #[test]
    fn huge_gives_up() {
        let big = "x\n".repeat(MAX_DIFF_LINES + 1);
        assert!(d("", &big).is_empty());
    }

    #[test]
    fn many_edits_fall_back_to_one_hunk() {
        let a: String = (0..5000).map(|i| format!("a{i}\n")).collect();
        let b: String = (0..5000).map(|i| format!("b{i}\n")).collect();
        assert_eq!(d(&a, &b), vec![(0..5000, Modified)]);
    }

    #[test]
    fn large_file_is_fast() {
        let a: String = (0..10_000).map(|i| format!("line {i}\n")).collect();
        let b = a.replace("line 500\n", "changed\n").replace("line 9000\n", "");
        let t = Instant::now();
        let c = d(&a, &b);
        assert_eq!(c, vec![(500..501, Modified), (9000..9000, Deleted)]);
        assert!(t.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn myers_finds_a_longest_common_subsequence() {
        let mut seed = 0x2545_f491_u64;
        let mut rnd = |n: u64| {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            (seed >> 33) % n
        };
        for _ in 0..300 {
            let a: Vec<u32> = (0..rnd(12)).map(|_| rnd(4) as u32).collect();
            let b: Vec<u32> = (0..rnd(12)).map(|_| rnd(4) as u32).collect();
            let m = myers(&a, &b).unwrap();
            assert!(m.windows(2).all(|w| w[0].0 < w[1].0 && w[0].1 < w[1].1));
            assert!(m.iter().all(|&(x, y)| a[x] == b[y]));
            let mut lcs = vec![vec![0; b.len() + 1]; a.len() + 1];
            for i in 1..=a.len() {
                for j in 1..=b.len() {
                    lcs[i][j] = if a[i - 1] == b[j - 1] { lcs[i - 1][j - 1] + 1 } else { lcs[i - 1][j].max(lcs[i][j - 1]) };
                }
            }
            assert_eq!(m.len(), lcs[a.len()][b.len()], "{a:?} {b:?}");
        }
    }

    #[test]
    fn next_change_wraps() {
        let c = vec![(2..4, Added), (10..10, Deleted), (20..21, Modified)];
        assert_eq!(next_change(&c, 0, true), Some(2));
        assert_eq!(next_change(&c, 2, true), Some(10));
        assert_eq!(next_change(&c, 25, true), Some(2));
        assert_eq!(next_change(&c, 25, false), Some(20));
        assert_eq!(next_change(&c, 3, false), Some(20));
        assert_eq!(next_change(&c, 12, false), Some(10));
        assert_eq!(next_change(&c, 1, false), Some(20));
        assert_eq!(next_change(&[], 1, true), None);
    }

    /// A throwaway repo with one committed file; None when git isn't installed.
    fn temp_repo(tag: &str) -> Option<PathBuf> {
        let dir = std::env::temp_dir().join(format!("strata-git-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).ok()?;
        let run = |args: &[&str]| git(&dir, args).is_some();
        if !run(&["init", "-q", "-b", "trunk"]) {
            return None;
        }
        std::fs::write(dir.join("sub/f.txt"), "one\ntwo\nthree\n").unwrap();
        assert!(run(&["add", "."]));
        assert!(run(&["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", "commit", "-q", "-m", "init"]));
        Some(dir)
    }

    #[test]
    fn repo_and_head() {
        let Some(dir) = temp_repo("head") else { return };
        let file = dir.join("sub/f.txt");
        std::fs::write(&file, "one\n2\nthree\nfour\n").unwrap();
        assert_eq!(head_text(&file).as_deref(), Some("one\ntwo\nthree\n"));
        let info = repo_info(&file).unwrap();
        assert_eq!(info.branch, "trunk");
        assert!(!info.detached);
        assert_eq!(info.root.canonicalize().unwrap(), dir.canonicalize().unwrap());
        let new = dir.join("sub/new.txt");
        std::fs::write(&new, "x").unwrap();
        assert_eq!(head_text(&new), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn outside_repo() {
        let dir = std::env::temp_dir().join(format!("strata-nogit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // The temp dir could itself sit inside a repo; only check it's quick and sane.
        let t = Instant::now();
        let _ = repo_info(&dir.join("x.txt"));
        assert!(t.elapsed() < GIT_TIMEOUT);
        assert_eq!(head_text(&dir.join("x.txt")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn watcher_returns_latest() {
        let Some(dir) = temp_repo("watch") else { return };
        let file = dir.join("sub/f.txt");
        let w = GitWatcher::new(None);
        w.request(&file, ropey::Rope::from_str("one\ntwo\n"), 1);
        w.request(&file, ropey::Rope::from_str("one\nTWO\nthree\n"), 2);
        let deadline = Instant::now() + Duration::from_secs(10);
        let got = loop {
            if let Some(r) = w.poll() {
                break r;
            }
            assert!(Instant::now() < deadline, "no result");
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(got, (file.clone(), 2, vec![(1..2, Modified)]));
        assert_eq!(w.repo(&file).map(|i| i.branch), Some("trunk".into()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn watcher_answers_every_file_asked_about() {
        // Split panes ask about several files in one frame: each gets its result.
        let Some(dir) = temp_repo("panes") else { return };
        let (a, b) = (dir.join("sub/f.txt"), dir.join("sub/g.txt"));
        std::fs::write(&b, "new\n").unwrap();
        let w = GitWatcher::new(None);
        w.request(&a, ropey::Rope::from_str("one\nTWO\nthree\n"), 1);
        w.request(&b, ropey::Rope::from_str("new\n"), 7);
        w.request(&a, ropey::Rope::from_str("one\nTWO\nthree\nfour\n"), 2);
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut got = Vec::new();
        while got.len() < 2 {
            while let Some(r) = w.poll() {
                got.push(r);
            }
            assert!(Instant::now() < deadline, "only got {got:?}");
            std::thread::sleep(Duration::from_millis(10));
        }
        got.sort_by_key(|r| r.1);
        assert_eq!(got, vec![(a.clone(), 2, vec![(1..2, Modified), (3..4, Added)]), (b.clone(), 7, vec![])]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
