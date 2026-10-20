//! Git sync for a notes vault (issue #12): commit what changed in the vault,
//! bring in the remote's commits with a rebase, push. Off until the user
//! turns it on for a vault (`notes_git_sync`); the window glue is
//! `app/notes_sync_ui.rs`.
//!
//! This is the one place Strata runs Git commands that change a repository
//! or reach the network, so it has its own command (`Repo::command`) apart
//! from the hardened read-only one in `crate::git`:
//!
//! - Git runs with the repository's own settings, as in a terminal: hooks,
//!   filters (Git LFS) and credential helpers run. Turning sync on asks first
//!   and says so; turning hooks off instead would be no protection (filters,
//!   `core.sshCommand` and credential helpers in the same config still run
//!   programs) and would break real setups (LFS uploads in its pre-push hook).
//!   Questions asked before that answer go through the hardened `crate::git`.
//! - Only the remote's own protocol is allowed (SSH, HTTPS or a local path),
//!   by name on the command line, which outranks the repository's config.
//! - Nothing prompts: no terminal prompts, `ssh -o BatchMode=yes` unless the
//!   user set up an ssh command of their own, no sign-in windows from Git
//!   Credential Manager. Credentials come from the user's helper or agent.
//! - Never a force push, `reset --hard` or discarded text: a rebase that
//!   stops is aborted (the files are as they were), and the notes in conflict
//!   are listed. Settling them ("Keep My Versions") records a merge that keeps
//!   the user's files as they are on disk, built without touching the working
//!   tree (`git merge-tree`).
//! - Git's error text can hold a remote's address with a password in it:
//!   it's shown and logged only through `redact`.

use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

/// Commands that may wait on a hook, a signing key or a big rebase.
const LOCAL_TIMEOUT: Duration = Duration::from_secs(120);
/// Fetch and push.
const NET_TIMEOUT: Duration = Duration::from_secs(180);
/// Quick questions (through the hardened `crate::git`).
const READ_TIMEOUT: Duration = Duration::from_secs(10);
/// Most output read from one command.
const OUT_LIMIT: usize = 16 << 20;
/// Every protocol Git knows by name; all but the remote's are turned off.
const PROTOCOLS: [&str; 6] = ["ssh", "ext", "file", "git", "http", "https"];
/// The default commit message (see `fill_message`).
pub const DEFAULT_MESSAGE: &str = "Notes sync: {count} changed, {date} {time}";

/// One vault's sync settings (`notes_git_sync` in settings.json).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(default)]
pub struct VaultSync {
    /// Sync this often while the vault is open; 0: only when asked.
    pub interval_minutes: u32,
    /// The repository sync was turned on for (see `Identity`).
    pub repository: Identity,
}

/// What makes a repository the one the user turned sync on for: another
/// repository later put at the same path doesn't inherit the permission to
/// run its hooks and config.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(default)]
pub struct Identity {
    /// The Git folder's file identity (device and inode; volume and file index on Windows).
    pub git_dir: String,
    /// The remote's address (without a password).
    pub remote: String,
    /// The first root commit.
    pub root: String,
}

/// The identity of the repository holding `vault` (read-only Git only).
pub fn identity(vault: &Path) -> std::result::Result<Identity, Failure> {
    let remote = describe(vault)?;
    identity_of(vault, &remote)
}

fn identity_of(vault: &Path, remote: &Remote) -> std::result::Result<Identity, Failure> {
    let missing = || fail(Kind::Untrusted, "Couldn't tell which repository this is");
    let dir = ask(vault, &["rev-parse", "--absolute-git-dir"]).ok_or_else(missing)?;
    let git_dir = file_id(Path::new(&dir)).ok_or_else(missing)?;
    let root = ask(vault, &["rev-list", "--max-parents=0", "HEAD"]).and_then(|r| r.lines().next().map(str::to_owned)).unwrap_or_default();
    Ok(Identity { git_dir, remote: remote.url.clone(), root })
}

/// A folder's identity on its disk: device and inode.
#[cfg(unix)]
fn file_id(path: &Path) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::metadata(path).ok()?;
    Some(format!("{}:{}", m.dev(), m.ino()))
}

/// A folder's identity on its disk: volume serial number and file index.
#[cfg(windows)]
fn file_id(path: &Path) -> Option<String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{BY_HANDLE_FILE_INFORMATION, CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, GetFileInformationByHandle, OPEN_EXISTING};
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    // SAFETY: a NUL-terminated path; the handle is closed before returning; `info` outlives the call.
    unsafe {
        let h = CreateFileW(wide.as_ptr(), 0, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, std::ptr::null(), OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS, std::ptr::null_mut());
        if h == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut info: BY_HANDLE_FILE_INFORMATION = std::mem::zeroed();
        let ok = GetFileInformationByHandle(h, &mut info);
        CloseHandle(h);
        (ok != 0).then(|| format!("{}:{}:{}", info.dwVolumeSerialNumber, info.nFileIndexHigh, info.nFileIndexLow))
    }
}

/// A file in the Git folder while a sync rewrites files: one left behind means Strata quit then.
const STARTED: &str = "strata-sync-started";

/// Is a sync of `vault` running (in any window)? (Tests.)
#[cfg(test)]
pub fn busy(vault: &Path) -> bool {
    let key = (false, std::fs::canonicalize(vault).unwrap_or_else(|_| vault.to_owned()));
    ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).contains(&key)
}

/// Strata is quitting: no sync starts rewriting files from now on, and one
/// that is rewriting them is waited for (until Git's own time limit has
/// passed; a rebase still left then is undone by the next sync, see `STARTED`).
pub fn shutdown() {
    {
        let _gate = GATE.lock().unwrap_or_else(|e| e.into_inner());
        SHUTDOWN.store(true, Ordering::SeqCst);
    }
    let deadline = Instant::now() + LOCAL_TIMEOUT + Duration::from_secs(30);
    while Instant::now() < deadline && HOLDS.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|(flag, _)| flag.load(Ordering::SeqCst)) {
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Set once Strata starts quitting (see `shutdown`).
static SHUTDOWN: AtomicBool = AtomicBool::new(false);
/// Held while a sync decides to start rewriting files, and while quitting
/// (or a window letting go of its sync) says no more may start: so one of
/// the two always sees the other.
static GATE: Mutex<()> = Mutex::new(());

/// Where a sync is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Check,
    Commit,
    Fetch,
    /// Rebase, merge or fast-forward: Git rewrites files in the working tree.
    Integrate,
    Push,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    NoGit,
    NotRepo,
    NoUpstream,
    Unsupported,
    /// A rebase or merge is already under way, or another Git holds a lock.
    Busy,
    /// Uncommitted changes outside the vault would stop the rebase.
    OutsideChanges,
    Offline,
    Auth,
    /// The remote moved on while syncing (twice).
    Rejected,
    Identity,
    /// The histories have nothing in common.
    Unrelated,
    /// Git failed some other way (a hook refused, say).
    Git,
    /// Git couldn't undo a rebase: the repository needs a look in Git.
    Stuck,
    /// Not the repository sync was turned on for (see `Identity`).
    Untrusted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub kind: Kind,
    /// What happened, for the status bar.
    pub message: String,
    /// Git's own last words (redacted), for a tooltip.
    pub detail: Option<String>,
}

/// Notes changed both here and on the remote: nothing was pulled or pushed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    /// The repository's working tree.
    pub top: PathBuf,
    /// "origin/main".
    pub upstream: String,
    /// The remote commit the notes conflict with (to compare against, and
    /// to check nothing new arrived before settling).
    pub commit: String,
    /// The conflicting files, relative to `top`, `/`-separated.
    pub paths: Vec<String>,
    /// Files written by another program while Git worked, saved as conflict
    /// copies before the rebase was undone.
    pub copies: Vec<PathBuf>,
    /// The remote changed the notes again since the last look.
    pub moved: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Result {
    /// Files committed, remote commits brought in, local commits pushed.
    Synced { committed: usize, pulled: usize, pushed: usize },
    Conflict(Conflict),
    Failed(Failure),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    /// Files committed before it ended (they stay committed whatever happened next).
    pub committed: usize,
    pub result: Result,
}

/// Settle a conflict by keeping the user's files (see `Conflict`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolution {
    pub paths: Vec<String>,
    pub commit: String,
}

#[derive(Clone, Default)]
pub struct Options {
    /// Commit message template (`fill_message`); empty: `DEFAULT_MESSAGE`.
    pub message: String,
    pub resolve: Option<Resolution>,
    /// Extra environment for Git (tests: a config of their own).
    pub env: Vec<(String, String)>,
    /// Called as each phase starts (tests).
    pub on_phase: Option<Arc<dyn Fn(Phase) + Send + Sync>>,
    /// How long the rebase or merge may take (tests; None: two minutes).
    pub integrate_timeout: Option<Duration>,
    /// The repository sync was turned on for: anything else is refused before Git runs.
    pub expect: Option<Identity>,
    /// Runs just before a stopped rebase is undone (tests).
    pub before_abort: Option<Arc<dyn Fn() + Send + Sync>>,
    /// Set when the window lets go of this sync: it mustn't start rewriting files then.
    pub cancel: Option<Arc<AtomicBool>>,
}

/// The vault's repository and remote, as `describe` finds them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Remote {
    pub top: PathBuf,
    /// The vault's folder below `top` ("Notes/"), or "".
    pub prefix: String,
    pub branch: String,
    /// The remote's name ("origin").
    pub name: String,
    /// The remote branch ("refs/heads/main").
    pub merge: String,
    /// The local copy of it ("refs/remotes/origin/main").
    pub tracking: String,
    /// "origin/main".
    pub upstream: String,
    /// The address, without any password, for showing.
    pub url: String,
    /// The protocols its fetch and push addresses use.
    pub protocols: Vec<&'static str>,
}

fn fail(kind: Kind, message: impl Into<String>) -> Failure {
    Failure { kind, message: message.into(), detail: None }
}

// ---- small helpers --------------------------------------------------------------

/// `text` (Git's or a hook's output) without the secrets it might hold:
/// passwords in addresses, Authorization and Bearer values, `token=`-style
/// pairs and the usual shapes of GitHub, GitLab, AWS, OpenAI and Anthropic keys.
pub fn redact(text: &str) -> String {
    static RULES: LazyLock<Vec<(regex::Regex, &'static str)>> = LazyLock::new(|| {
        [
            (r"(?i)\b([a-z][a-z0-9+.-]*://)[^/@\s]+@", "$1"),
            (r"(?i)\b((?:proxy-)?authorization)\s*[:=]\s*(?:(?:basic|bearer|token|digest)\s+)?[^\s,;]+", "$1: [redacted]"),
            (r"(?i)\bbearer\s+[A-Za-z0-9._~+/=-]{6,}", "Bearer [redacted]"),
            (r#"(?i)\b((?:access_|refresh_|auth_|private_)?token|passw(?:or)?d|pwd|secret|client_secret|api[_-]?key)(["']?\s*[:=]\s*)["']?[^\s&"',;]+["']?"#, "$1$2[redacted]"),
            (r"\bsk-ant-[A-Za-z0-9_-]{8,}", "[redacted]"),
            (r"\bsk-[A-Za-z0-9_-]{16,}", "[redacted]"),
            (r"\bgh[pousr]_[A-Za-z0-9]{16,}", "[redacted]"),
            (r"\bgithub_pat_[A-Za-z0-9_]{16,}", "[redacted]"),
            (r"\bglpat-[A-Za-z0-9_-]{12,}", "[redacted]"),
            (r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b", "[redacted]"),
        ]
        .into_iter()
        .map(|(re, with)| (regex::Regex::new(re).expect("valid"), with))
        .collect()
    });
    let mut out = text.to_owned();
    for (re, with) in RULES.iter() {
        out = re.replace_all(&out, *with).into_owned();
    }
    out
}

/// The protocol Git would use for `url`, if sync allows it.
pub fn protocol_of(url: &str) -> std::result::Result<&'static str, String> {
    let u = url.trim();
    let lower = u.to_ascii_lowercase();
    if let Some(i) = u.find("::")
        && !u[..i].contains(['/', '\\', ':'])
    {
        return Err(format!("its address uses a Git remote helper (“{}::”), which Strata doesn't run", &u[..i]));
    }
    for (scheme, proto) in [("https://", "https"), ("ssh://", "ssh"), ("git+ssh://", "ssh"), ("ssh+git://", "ssh"), ("file://", "file")] {
        if lower.starts_with(scheme) {
            return Ok(proto);
        }
    }
    if lower.starts_with("http://") || lower.starts_with("git://") {
        return Err("its address isn't encrypted (http:// or git://); use SSH or HTTPS".into());
    }
    if let Some(i) = lower.find("://") {
        return Err(format!("Strata syncs over SSH, HTTPS or a local path, not {}://", &u[..i]));
    }
    // Git's scp-like form: no slash before the first colon (and not a drive letter).
    if let Some(colon) = u.find(':') {
        let before = &u[..colon];
        let drive = before.len() == 1 && before.as_bytes()[0].is_ascii_alphabetic();
        if !before.contains(['/', '\\']) && !drive {
            return Ok("ssh");
        }
    }
    Ok("file")
}

/// The commit message: `{count}` changed files, `{date}` (YYYY-MM-DD) and `{time}` (HH:mm).
pub fn fill_message(template: &str, count: usize, now: &super::dates::DateTime) -> String {
    let t = if template.trim().is_empty() { DEFAULT_MESSAGE } else { template };
    t.replace("{count}", &count.to_string()).replace("{date}", &super::dates::format("YYYY-MM-DD", now)).replace("{time}", &super::dates::format("HH:mm", now))
}

/// What a failed Git command's error output means.
pub fn classify(stderr: &str) -> Kind {
    let e = stderr.to_ascii_lowercase();
    let any = |words: &[&str]| words.iter().any(|w| e.contains(w));
    if any(&["please tell me who you are", "empty ident", "unable to auto-detect email"]) {
        Kind::Identity
    } else if any(&["authentication failed", "permission denied", "terminal prompts disabled", "could not read username", "could not read password", "host key verification failed", "invalid username or password", "the requested url returned error: 401", "the requested url returned error: 403"]) {
        Kind::Auth
    } else if any(&["[rejected]", "non-fast-forward", "fetch first", "failed to push some refs"]) {
        Kind::Rejected
    } else if any(&["index.lock", ".lock': file exists", "another git process"]) {
        Kind::Busy
    } else if any(&[
        "could not resolve host",
        "could not resolve hostname",
        "connection refused",
        "connection timed out",
        "operation timed out",
        "network is unreachable",
        "no route to host",
        "failed to connect",
        "couldn't connect",
        "could not read from remote repository",
        "does not appear to be a git repository",
        "unable to access",
        "connection reset",
        "temporary failure in name resolution",
    ]) {
        Kind::Offline
    } else {
        Kind::Git
    }
}

/// The line of `stderr` worth showing (the first "ssh:", "fatal:" or
/// "error:" line, else the last one), without passwords, at most 300 chars.
pub fn last_words(stderr: &str) -> Option<String> {
    let lines: Vec<&str> = stderr.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with("hint:")).collect();
    let line = lines.iter().find(|l| ["ssh:", "fatal:", "error:"].iter().any(|p| l.starts_with(p))).or(lines.last())?;
    Some(redact(line).chars().take(300).collect())
}

/// A failure from Git's error output, said for people.
fn git_failure(stderr: &str, doing: &str) -> Failure {
    let kind = classify(stderr);
    let message = match kind {
        Kind::Identity => "Git doesn't know your name and email yet: set them with git config user.name and user.email".to_owned(),
        Kind::Auth => "The remote refused the sign-in. Check that Git can push from a terminal (Strata uses your credential helper or SSH agent, and never asks for a password)".to_owned(),
        Kind::Rejected => "The remote changed while syncing; sync again".to_owned(),
        Kind::Busy => "Another Git command is working in this repository; try again in a moment".to_owned(),
        Kind::Offline => "Couldn't reach the remote (offline?)".to_owned(),
        _ if ["unstaged changes", "would be overwritten"].iter().any(|w| stderr.contains(w)) => format!("Git couldn't {doing}: a file changed while it worked (saved meanwhile?); sync again"),
        _ => format!("Git couldn't {doing}"),
    };
    Failure { kind, message, detail: last_words(stderr) }
}

struct Out {
    code: Option<i32>,
    stdout: Vec<u8>,
    stderr: String,
}

impl Out {
    fn ok(&self) -> bool {
        self.code == Some(0)
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).trim().to_owned()
    }
}

/// Run `cmd` with nothing on its input, reading both outputs, ended at `timeout`.
fn exec(mut cmd: Command, timeout: Duration) -> std::result::Result<Out, Failure> {
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    // Its own process group, so a timeout stops Git's hooks, ssh and filters too.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    let mut child = cmd.spawn().map_err(|e| fail(Kind::NoGit, format!("Couldn't run Git ({e}); is it installed?")))?;
    // Windows has no process groups: a job object holds Git and what it starts.
    #[cfg(windows)]
    let job = crate::lsp::server::job::Job::holding(&child);
    // Readers send what they got: a hook's background process may keep a
    // pipe open after Git exits, and that mustn't hold the sync up.
    let (tx, rx) = channel();
    for (k, pipe) in [(0u8, child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>)), (1, child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>))] {
        let Some(pipe) = pipe else { continue };
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = pipe.take(OUT_LIMIT as u64).read_to_end(&mut buf);
            let _ = tx.send((k, buf));
        });
    }
    drop(tx);
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            _ => {
                #[cfg(unix)]
                if let Ok(pid) = libc::pid_t::try_from(child.id()) {
                    // SAFETY: signals the process group made for this child above.
                    unsafe { libc::kill(-pid, libc::SIGKILL) };
                }
                #[cfg(windows)]
                if let Some(job) = &job {
                    job.terminate();
                }
                let _ = child.kill();
                let _ = child.wait();
                return Err(fail(Kind::Offline, format!("Git took longer than {} s and was stopped", timeout.as_secs())));
            }
        }
    };
    // Finished in time: what it left running (a hook's own background work) goes on.
    #[cfg(windows)]
    if let Some(job) = job {
        job.release();
    }
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let until = Instant::now() + Duration::from_secs(5);
    for _ in 0..2 {
        match rx.recv_timeout(until.saturating_duration_since(Instant::now())) {
            Ok((0, b)) => stdout = b,
            Ok((_, b)) => stderr = b,
            Err(_) => break,
        }
    }
    Ok(Out { code: status.code(), stdout, stderr: String::from_utf8_lossy(&stderr).into_owned() })
}

/// stdout of a read-only question through the hardened `crate::git`, on success.
fn ask(dir: &Path, args: &[&str]) -> Option<String> {
    match crate::git::git_io_code(dir, args, None, OUT_LIMIT, READ_TIMEOUT) {
        Some((Some(0), out)) => Some(String::from_utf8_lossy(&out).trim().to_owned()),
        _ => None,
    }
}

/// NUL-separated names in `out`.
fn names(out: &[u8]) -> Vec<String> {
    out.split(|&b| b == 0).filter(|s| !s.is_empty()).map(|s| String::from_utf8_lossy(s).into_owned()).collect()
}

// ---- the remote ------------------------------------------------------------------

/// The repository and remote `vault` would sync with, or why it can't.
/// Runs only read-only, hardened Git (it's asked before sync is turned on).
pub fn describe(vault: &Path) -> std::result::Result<Remote, Failure> {
    let not_repo = || fail(Kind::NotRepo, "This folder isn't in a Git repository. Set one up with a remote first (see Notes ▸ Git sync in the manual)");
    if ask(vault, &["rev-parse", "--is-inside-work-tree"]).as_deref() != Some("true") {
        return Err(not_repo());
    }
    let top = PathBuf::from(ask(vault, &["rev-parse", "--show-toplevel"]).ok_or_else(not_repo)?);
    let prefix = ask(vault, &["rev-parse", "--show-prefix"]).unwrap_or_default();
    // Mid-rebase HEAD is detached; the branch being rebased is the one (a
    // rebase left by a quit Strata is undone by the sync, see `STARTED`).
    let rebasing = || {
        ["rebase-merge/head-name", "rebase-apply/head-name"].iter().find_map(|f| ask(vault, &["rev-parse", "--git-path", f]).and_then(|p| std::fs::read_to_string(vault.join(p)).ok()))
            .and_then(|r| r.trim().strip_prefix("refs/heads/").map(str::to_owned))
    };
    let Some(branch) = ask(vault, &["symbolic-ref", "-q", "--short", "HEAD"]).filter(|b| !b.is_empty()).or_else(rebasing) else {
        return Err(fail(Kind::NoUpstream, "No branch is checked out (HEAD is detached); check out a branch in Git first"));
    };
    let no_upstream = || fail(Kind::NoUpstream, format!("The branch “{branch}” has no upstream on a remote. Set one in Git (git push -u origin {branch}) and sync again"));
    let name = ask(vault, &["config", "--get", &format!("branch.{branch}.remote")]).filter(|n| !n.is_empty()).ok_or_else(no_upstream)?;
    let merge = ask(vault, &["config", "--get", &format!("branch.{branch}.merge")]).filter(|m| !m.is_empty()).ok_or_else(no_upstream)?;
    if name == "." {
        return Err(fail(Kind::NoUpstream, format!("The branch “{branch}” follows a local branch, not a remote; set an upstream on a remote in Git")));
    }
    let tracking = ask(vault, &["for-each-ref", "--format=%(upstream)", &format!("refs/heads/{branch}")]).filter(|t| t.starts_with("refs/remotes/")).ok_or_else(no_upstream)?;
    let upstream = tracking.trim_start_matches("refs/remotes/").to_owned();
    let fetch = ask(vault, &["ls-remote", "--get-url", &name]).filter(|u| !u.is_empty() && *u != name).ok_or_else(|| fail(Kind::NoUpstream, format!("The remote “{name}” has no address")))?;
    let push = ask(vault, &["remote", "get-url", "--push", &name]).unwrap_or_else(|| fetch.clone());
    let mut protocols = Vec::new();
    for url in [&fetch, &push] {
        let p = protocol_of(url).map_err(|why| fail(Kind::Unsupported, format!("Can't sync with “{name}”: {why}")))?;
        if !protocols.contains(&p) {
            protocols.push(p);
        }
    }
    Ok(Remote { top, prefix, branch, name, merge, tracking, upstream, url: redact(&fetch), protocols })
}

// ---- running a sync ----------------------------------------------------------------

struct Repo<'a> {
    vault: PathBuf,
    remote: Remote,
    opts: &'a Options,
    /// Use `ssh -o BatchMode=yes` (no ssh command of the user's own).
    batch_ssh: bool,
}

impl Repo<'_> {
    /// Git for the sync's own steps (see the module comment).
    fn command(&self, dir: &Path, args: &[&str]) -> Command {
        let mut cmd = Command::new("git");
        cmd.args(["-c", "protocol.allow=never"]);
        for p in PROTOCOLS {
            let allow = if self.remote.protocols.contains(&p) { "always" } else { "never" };
            cmd.args(["-c", &format!("protocol.{p}.allow={allow}")]);
        }
        // The working tree is committed before a rebase: nothing to stash.
        for c in ["submodule.recurse=false", "core.quotepath=false", "color.ui=false", "rebase.autoStash=false", "merge.autoStash=false", "rebase.updateRefs=false", "advice.detachedHead=false"] {
            cmd.args(["-c", c]);
        }
        cmd.arg("--no-pager").args(args).current_dir(dir);
        // Git started from a hook (or a terminal inside one) must not aim at that repository instead.
        for v in ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_OBJECT_DIRECTORY", "GIT_ALTERNATE_OBJECT_DIRECTORIES", "GIT_COMMON_DIR", "GIT_NAMESPACE", "GIT_CONFIG_PARAMETERS", "GIT_CONFIG_COUNT", "GIT_PREFIX"] {
            cmd.env_remove(v);
        }
        // An empty GIT_ASKPASS also outranks core.askPass and SSH_ASKPASS:
        // no password window either.
        cmd.env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_ASKPASS", "")
            .env("SSH_ASKPASS", "")
            .env("GCM_INTERACTIVE", "never")
            .env("SSH_ASKPASS_REQUIRE", "never")
            .env("GIT_EDITOR", ":")
            .env("GIT_SEQUENCE_EDITOR", ":")
            .env("GIT_MERGE_AUTOEDIT", "no")
            .env("LC_ALL", "C");
        if self.batch_ssh {
            cmd.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
        }
        for (k, v) in &self.opts.env {
            cmd.env(k, v);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        cmd
    }

    fn run(&self, dir: &Path, args: &[&str], timeout: Duration) -> std::result::Result<Out, Failure> {
        exec(self.command(dir, args), timeout)
    }

    /// `run`, failing with Git's reason unless it exits 0.
    fn must(&self, dir: &Path, args: &[&str], timeout: Duration, doing: &str) -> std::result::Result<Out, Failure> {
        let out = self.run(dir, args, timeout)?;
        if out.ok() { Ok(out) } else { Err(git_failure(&out.stderr, doing)) }
    }

    fn top(&self) -> &Path {
        &self.remote.top
    }

    fn rev(&self, spec: &str) -> Option<String> {
        ask(self.top(), &["rev-parse", "-q", "--verify", &format!("{spec}^{{commit}}")])
    }

    fn count(&self, range: &str) -> usize {
        ask(self.top(), &["rev-list", "--count", range]).and_then(|n| n.parse().ok()).unwrap_or(0)
    }

    /// A Git file (`rebase-merge`, `MERGE_HEAD`...) exists.
    fn git_path_exists(&self, name: &str) -> bool {
        ask(&self.vault, &["rev-parse", "--git-path", name]).is_some_and(|p| self.vault.join(p).exists())
    }

    fn in_progress(&self) -> bool {
        ["rebase-merge", "rebase-apply", "MERGE_HEAD", "CHERRY_PICK_HEAD", "REVERT_HEAD"].iter().any(|n| self.git_path_exists(n))
    }

    /// Is `rel` (from the top) inside the vault?
    fn in_vault(&self, rel: &str) -> bool {
        rel.starts_with(&self.remote.prefix)
    }

    /// Nothing in the way of a rebase: no operation under way, no conflict
    /// left over, no uncommitted change outside the vault.
    fn check_clean(&self) -> std::result::Result<(), Failure> {
        if self.in_progress() {
            return Err(fail(Kind::Busy, "A rebase or merge is under way in this repository; finish it in Git first. Nothing was changed"));
        }
        let out = self.must(self.top(), &["status", "--porcelain=v1", "-z", "--untracked-files=no", "--no-renames"], LOCAL_TIMEOUT, "read the repository's status")?;
        let mut outside = Vec::new();
        for entry in names(&out.stdout) {
            let (xy, path) = entry.split_at(entry.len().min(3));
            if xy.contains('U') || xy.starts_with("AA") || xy.starts_with("DD") {
                return Err(fail(Kind::Busy, format!("{path} has an unresolved conflict; settle it in Git first. Nothing was changed")));
            }
            if !self.in_vault(path) {
                outside.push(path.to_owned());
            }
        }
        if !outside.is_empty() {
            let shown: Vec<&str> = outside.iter().take(3).map(String::as_str).collect();
            let more = if outside.len() > 3 { format!(" and {} more", outside.len() - 3) } else { String::new() };
            return Err(fail(Kind::OutsideChanges, format!("Files outside the notes folder have uncommitted changes ({}{more}); commit or stash them in Git first. Nothing was changed", shown.join(", "))));
        }
        Ok(())
    }

    /// Commit the vault's changes; how many files changed.
    fn commit(&self) -> std::result::Result<usize, Failure> {
        let vault = &self.vault;
        self.must(vault, &["add", "-A", "--", "."], LOCAL_TIMEOUT, "add the changed notes")?;
        let staged = self.must(vault, &["diff", "--cached", "--name-only", "-z", "--no-renames", "--", "."], LOCAL_TIMEOUT, "list the changed notes")?;
        let count = names(&staged.stdout).len();
        if count == 0 {
            return Ok(0);
        }
        let msg = fill_message(&self.opts.message, count, &super::dates::DateTime::now());
        // Only the vault's paths: anything else staged stays out of the commit.
        let out = self.run(vault, &["commit", "-q", "-m", &msg, "--", "."], LOCAL_TIMEOUT)?;
        if !out.ok() {
            let mut f = git_failure(&out.stderr, "commit the notes");
            if f.kind == Kind::Git {
                f.message = "Git couldn't commit the notes (a commit hook may have refused); nothing was pulled or pushed".into();
            }
            return Err(f);
        }
        Ok(count)
    }

    fn fetch(&self) -> std::result::Result<(), Failure> {
        let out = self.run(&self.vault, &["fetch", "-q", "--no-recurse-submodules", &self.remote.name], NET_TIMEOUT)?;
        if out.ok() { Ok(()) } else { Err(git_failure(&out.stderr, "fetch from the remote")) }
    }

    /// Bring in the remote's commits; `Ok(None)` when done, a conflict otherwise.
    fn integrate(&self, behind: usize, ahead: usize, phase: &dyn Fn(Phase)) -> std::result::Result<Option<Conflict>, Failure> {
        if behind == 0 {
            return Ok(None);
        }
        let tracking = self.remote.tracking.as_str();
        let before = self.rev("HEAD").ok_or_else(|| fail(Kind::Git, "Couldn't read HEAD"))?;
        // Notes changed on both sides are found in Git's objects first, so
        // the usual conflict never touches the working tree (nor leaves
        // conflict markers behind if Strata quits halfway). A rebase can
        // still stop on a commit in between; that's undone below.
        if ahead > 0
            && let Some(paths) = self.would_conflict(tracking)
            && !paths.is_empty()
        {
            let commit = self.rev(tracking).unwrap_or_default();
            return Ok(Some(Conflict { top: self.top().to_owned(), upstream: self.remote.upstream.clone(), commit, paths, copies: Vec::new(), moved: false }));
        }
        let _started = self.enter(phase)?;
        let merge = ahead > 0 && ask(self.top(), &["rev-list", "--merges", "--count", &format!("{tracking}..HEAD")]).is_some_and(|n| n != "0");
        let args: Vec<&str> = if ahead == 0 {
            vec!["merge", "-q", "--ff-only", tracking]
        } else if merge {
            // A merge of Strata's own (Keep My Versions) that didn't get pushed:
            // a rebase would drop it and replay the conflict.
            vec!["merge", "-q", "--no-edit", tracking]
        } else {
            vec!["rebase", "-q", tracking]
        };
        let out = match self.run(self.top(), &args, self.opts.integrate_timeout.unwrap_or(LOCAL_TIMEOUT)) {
            Ok(out) => out,
            // Stopped (too slow): undo whatever it began, as for a failure.
            Err(f) => Out { code: None, stdout: Vec::new(), stderr: format!("error: {}", f.message) },
        };
        if out.ok() {
            return Ok(None);
        }
        if !self.in_progress() {
            // Git stopped before it began (a file in the way, a hook): nothing changed.
            let mut f = git_failure(&out.stderr, "bring in the remote's changes");
            if self.rev("HEAD").as_deref() != Some(before.as_str()) || out.code.is_none() {
                f.kind = Kind::Stuck;
                f.message = "Git stopped partway through bringing in the remote's changes; look at the repository in Git (git status). Nothing was pushed".into();
            }
            return Err(f);
        }
        if let Some(f) = &self.opts.before_abort {
            f();
        }
        let unmerged = self.unmerged();
        let copies = self.save_strays(&unmerged, Some(&before));
        let kept = if copies.is_empty() {
            String::new()
        } else {
            let names: Vec<String> = copies.iter().map(|c| c.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()).collect();
            format!(". Text another program wrote meanwhile was kept as {}", names.join(", "))
        };
        let abort: &[&str] = if args[0] == "rebase" { &["rebase", "--abort"] } else { &["merge", "--abort"] };
        let aborted = self.run(self.top(), abort, LOCAL_TIMEOUT);
        let undone = aborted.as_ref().is_ok_and(|o| o.ok());
        if !undone || self.in_progress() || self.rev("HEAD").as_deref() != Some(before.as_str()) {
            log::warn!("notes sync: couldn't undo the {} in {}", args[0], self.top().display());
            let why = match &aborted {
                Ok(o) => last_words(&o.stderr),
                Err(f) => Some(f.message.clone()),
            };
            return Err(Failure { kind: Kind::Stuck, message: format!("Git couldn't undo the {} it started; look at the repository in Git (git status). Your notes' text is in Git's history{kept}", args[0]), detail: why.or_else(|| last_words(&out.stderr)) });
        }
        if unmerged.is_empty() {
            // Stopped for another reason (a file changed under it, say), and undone.
            let mut f = git_failure(&out.stderr, "bring in the remote's changes");
            f.message = format!("{}; your files are as they were{kept}", f.message);
            return Err(f);
        }
        let commit = self.rev(tracking).unwrap_or_default();
        Ok(Some(Conflict { top: self.top().to_owned(), upstream: self.remote.upstream.clone(), commit, paths: unmerged.into_iter().map(|(p, _)| p).collect(), copies, moved: false }))
    }

    /// Files this side ignores (so they aren't committed) that the remote's
    /// commits would bring in: Git would overwrite them without asking.
    fn ignored_in_the_way(&self, tracking: &str) -> std::result::Result<(), Failure> {
        let Ok(out) = self.run(self.top(), &["ls-files", "-z", "--others", "--ignored", "--exclude-standard", "--directory"], LOCAL_TIMEOUT) else { return Ok(()) };
        let ignored = names(&out.stdout);
        if ignored.is_empty() {
            return Ok(());
        }
        let incoming = crate::git::git_io_code(self.top(), &["diff-tree", "-r", "--name-only", "-z", "--no-renames", "HEAD", tracking], None, OUT_LIMIT, READ_TIMEOUT).filter(|(c, _)| *c == Some(0)).map(|(_, o)| names(&o)).unwrap_or_default();
        let hit: Vec<&String> = incoming.iter().filter(|p| ignored.iter().any(|i| *p == i || (i.ends_with('/') && p.starts_with(i.as_str())))).collect();
        if hit.is_empty() {
            return Ok(());
        }
        let shown: Vec<&str> = hit.iter().take(3).map(|s| s.as_str()).collect();
        let more = if hit.len() > 3 { format!(" and {} more", hit.len() - 3) } else { String::new() };
        Err(fail(Kind::OutsideChanges, format!("{} would replace files this computer ignores ({}{more}); move them aside first. Nothing was pulled or pushed", self.remote.upstream, shown.join(", "))))
    }

    /// The files a merge of HEAD and `tracking` would leave in conflict
    /// (none: it merges cleanly); None when Git can't tell (before 2.38).
    fn would_conflict(&self, tracking: &str) -> Option<Vec<String>> {
        let out = self.run(self.top(), &["merge-tree", "--write-tree", "--name-only", "-z", "--no-messages", "HEAD", tracking], LOCAL_TIMEOUT).ok()?;
        match out.code {
            Some(0) => Some(Vec::new()),
            Some(1) => {
                let mut paths: Vec<String> = names(&out.stdout).into_iter().skip(1).collect();
                paths.dedup();
                Some(paths)
            }
            _ => None,
        }
    }

    /// Files another program wrote while Git was stopped mid-rebase (they
    /// differ from Git's index, and aren't Git's own conflicts): undoing the
    /// rebase would put back the committed text over them, so each is
    /// copied next to itself first, named as a conflict copy.
    /// A conflicted file is checked too: one that isn't exactly what Git
    /// wrote there (a side, or the merge with conflict markers) was saved by
    /// something else.
    /// Copies never take a name tracked in `back_to` (the commit the abort
    /// returns to): the abort would refuse to put that file back.
    fn save_strays(&self, unmerged: &[(String, Stages)], back_to: Option<&str>) -> Vec<PathBuf> {
        let taken = |copy: &Path| {
            let Some(commit) = back_to else { return false };
            let Ok(rel) = copy.strip_prefix(self.top()) else { return false };
            let rel = rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
            crate::git::git_io_code(self.top(), &["cat-file", "-e", &format!("{commit}:{rel}")], None, 1 << 10, READ_TIMEOUT).is_some_and(|(c, _)| c == Some(0))
        };
        let mut todo: Vec<String> = unmerged.iter().filter(|(rel, st)| !self.git_wrote(rel, st)).map(|(rel, _)| rel.clone()).collect();
        // Fails closed: if Git can't say what changed, every touched file is kept aside.
        match self.run(self.top(), &["diff", "--name-only", "-z", "--no-renames"], LOCAL_TIMEOUT) {
            Ok(out) if out.ok() => todo.extend(names(&out.stdout).into_iter().filter(|r| !unmerged.iter().any(|(u, _)| u == r))),
            _ => log::warn!("notes sync: couldn't list the files changed during the rebase"),
        }
        let mut copies = Vec::new();
        for rel in todo {
            let path = self.top().join(&rel);
            if !std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_file()) {
                continue;
            }
            match copy_aside(&path, &taken) {
                Ok(c) => copies.push(c),
                Err(e) => log::warn!("notes sync: couldn't keep a copy of {}: {e}", path.display()),
            }
        }
        copies
    }

    /// Files Git left in conflict, with the blobs of their stages.
    fn unmerged(&self) -> Vec<(String, Stages)> {
        let mut out: Vec<(String, Stages)> = Vec::new();
        for e in ask(self.top(), &["ls-files", "-u", "-z"]).unwrap_or_default().split('\0') {
            let Some((meta, path)) = e.split_once('\t') else { continue };
            let f: Vec<&str> = meta.split(' ').collect();
            let [_, id, stage] = f[..] else { continue };
            let Some(k) = stage.parse::<usize>().ok().filter(|k| (1..=3).contains(k)) else { continue };
            if out.last().is_none_or(|(p, _)| p != path) {
                out.push((path.to_owned(), Stages::default()));
            }
            if let Some(last) = out.last_mut() {
                last.1[k - 1] = Some(id.to_owned());
            }
        }
        out
    }

    /// Is the file `rel` exactly what Git wrote while stopping: one of its
    /// stages, or their merge with conflict markers (labels aside)?
    fn git_wrote(&self, rel: &str, stages: &Stages) -> bool {
        let Ok(work) = std::fs::read(self.top().join(rel)) else { return true };
        let blob = |id: &Option<String>| id.as_deref().and_then(|id| crate::git::git_io(self.top(), &["cat-file", "blob", id], None, OUT_LIMIT, READ_TIMEOUT));
        let blobs: [Option<Vec<u8>>; 3] = [blob(&stages[0]), blob(&stages[1]), blob(&stages[2])];
        if blobs.iter().flatten().any(|b| *b == work) {
            return true;
        }
        let (Some(ours), Some(theirs)) = (&blobs[1], &blobs[2]) else { return false };
        let Some(dir) = ask(&self.vault, &["rev-parse", "--git-path", "strata-sync-merge"]).map(|p| self.vault.join(p)) else { return false };
        if std::fs::create_dir_all(&dir).is_err() {
            return false;
        }
        let files = [("ours", ours.as_slice()), ("base", blobs[0].as_deref().unwrap_or_default()), ("theirs", theirs.as_slice())];
        if files.iter().any(|(name, bytes)| std::fs::write(dir.join(name), bytes).is_err()) {
            let _ = std::fs::remove_dir_all(&dir);
            return false;
        }
        let mut args = vec!["merge-file", "-p"];
        match ask(self.top(), &["config", "--get", "merge.conflictStyle"]).as_deref() {
            Some("diff3") => args.push("--diff3"),
            Some("zdiff3") => args.push("--zdiff3"),
            _ => {}
        }
        let paths: Vec<String> = ["ours", "base", "theirs"].iter().map(|n| dir.join(n).display().to_string()).collect();
        args.extend(paths.iter().map(String::as_str));
        let merged = self.run(self.top(), &args, LOCAL_TIMEOUT).ok().filter(|o| matches!(o.code, Some(c) if (0..128).contains(&c))).map(|o| o.stdout);
        let _ = std::fs::remove_dir_all(&dir);
        merged.is_some_and(|m| without_labels(&m) == without_labels(&work))
    }

    /// From here on Git rewrites files: unless Strata is quitting, or the
    /// window let go of this sync. Leaves `STARTED` in the Git folder until
    /// the guard goes, so a quit halfway is undone by the next sync.
    fn enter(&self, phase: &dyn Fn(Phase)) -> std::result::Result<Started, Failure> {
        let _gate = GATE.lock().unwrap_or_else(|e| e.into_inner());
        if SHUTDOWN.load(Ordering::SeqCst) || self.opts.cancel.as_ref().is_some_and(|c| c.load(Ordering::SeqCst)) {
            return Err(fail(Kind::Busy, "Sync stopped before bringing in the remote's changes (Strata is quitting); nothing was pulled or pushed"));
        }
        let marker = ask(&self.vault, &["rev-parse", "--git-path", STARTED]).map(|p| self.vault.join(p));
        if let Some(m) = &marker {
            let _ = std::fs::write(m, b"");
        }
        phase(Phase::Integrate);
        Ok(Started(marker))
    }

    /// A rebase or merge left behind by a sync Strata quit during (`STARTED`
    /// is there): undone, keeping aside what other programs wrote meanwhile.
    fn recover(&self) -> std::result::Result<(), Failure> {
        let Some(marker) = ask(&self.vault, &["rev-parse", "--git-path", STARTED]).map(|p| self.vault.join(p)).filter(|m| m.exists()) else { return Ok(()) };
        if !self.in_progress() {
            let _ = std::fs::remove_file(&marker);
            return Ok(());
        }
        let unmerged = self.unmerged();
        let back_to = self.rev("ORIG_HEAD");
        let copies = self.save_strays(&unmerged, back_to.as_deref());
        let abort: &[&str] = if self.git_path_exists("MERGE_HEAD") { &["merge", "--abort"] } else { &["rebase", "--abort"] };
        let undone = self.run(self.top(), abort, LOCAL_TIMEOUT).is_ok_and(|o| o.ok()) && !self.in_progress();
        if !undone {
            return Err(fail(Kind::Stuck, "A sync that was stopped when Strata quit left a rebase that Git couldn't undo; look at the repository in Git (git status)"));
        }
        let _ = std::fs::remove_file(&marker);
        let kept = if copies.is_empty() { String::new() } else { format!(" Text written meanwhile was kept as {}.", copies.iter().filter_map(|c| c.file_name()).map(|n| n.to_string_lossy().into_owned()).collect::<Vec<_>>().join(", ")) };
        Err(fail(Kind::Git, format!("A sync that was stopped when Strata quit has been undone; your files are as they were.{kept} Sync again")))
    }

    /// Keep My Versions: a merge commit with the remote whose conflicting
    /// files are this side's, built in a scratch index so the working tree
    /// only ever sees the finished result.
    fn settle(&self, r: &Resolution, phase: &dyn Fn(Phase)) -> std::result::Result<Option<Conflict>, Failure> {
        let tracking = self.remote.tracking.as_str();
        let theirs = self.rev(tracking).ok_or_else(|| fail(Kind::Git, "The remote branch is gone; nothing was changed"))?;
        let conflict = |paths: Vec<String>, moved: bool| Conflict { top: self.top().to_owned(), upstream: self.remote.upstream.clone(), commit: theirs.clone(), paths, copies: Vec::new(), moved };
        if theirs != r.commit {
            let mut args = vec!["diff-tree", "-r", "--name-only", "-z", "--no-renames", r.commit.as_str(), theirs.as_str(), "--"];
            args.extend(r.paths.iter().map(String::as_str));
            let changed = crate::git::git_io_code(self.top(), &args, None, OUT_LIMIT, READ_TIMEOUT).filter(|(c, _)| *c == Some(0)).map(|(_, o)| names(&o));
            if changed.is_none_or(|c| !c.is_empty()) {
                return Ok(Some(conflict(r.paths.clone(), true)));
            }
        }
        let out = self.run(self.top(), &["merge-tree", "--write-tree", "--name-only", "-z", "--no-messages", "HEAD", tracking], LOCAL_TIMEOUT)?;
        if !matches!(out.code, Some(0) | Some(1)) {
            return Err(Failure { kind: Kind::Git, message: "Git couldn't work out the merge (Keep My Versions needs Git 2.38 or later)".into(), detail: last_words(&out.stderr) });
        }
        let mut parts = names(&out.stdout).into_iter();
        let tree = parts.next().ok_or_else(|| fail(Kind::Git, "Git didn't say how the merge looks"))?;
        // (Its conflicting files are listed after the tree.)
        let mut conflicted: Vec<String> = parts.collect();
        conflicted.dedup();
        let new: Vec<String> = conflicted.iter().filter(|p| !r.paths.contains(p)).cloned().collect();
        if !new.is_empty() {
            let mut all = r.paths.clone();
            all.extend(new);
            return Ok(Some(conflict(all, true)));
        }
        let index = ask(&self.vault, &["rev-parse", "--git-path", "strata-sync-index"]).map(|p| self.vault.join(p)).ok_or_else(|| fail(Kind::Git, "Couldn't find the repository's Git folder"))?;
        let _ = std::fs::remove_file(&index);
        let result = self.settle_commit(&index, &tree, &conflicted, &theirs);
        let _ = std::fs::remove_file(&index);
        let commit = result?;
        let _started = self.enter(phase)?;
        let before = self.rev("HEAD");
        if let Err(mut f) = self.must(self.top(), &["merge", "-q", "--ff-only", &commit], LOCAL_TIMEOUT, "update the notes to the merge") {
            if self.rev("HEAD") != before || f.message.contains("was stopped") {
                f.kind = Kind::Stuck;
                f.message = "Git stopped partway through updating the notes; look at the repository in Git (git status). Nothing was pushed".into();
            }
            return Err(f);
        }
        Ok(None)
    }

    fn settle_commit(&self, index: &Path, tree: &str, conflicted: &[String], theirs: &str) -> std::result::Result<String, Failure> {
        let with_index = |args: &[&str]| {
            let mut cmd = self.command(self.top(), args);
            cmd.env("GIT_INDEX_FILE", index);
            exec(cmd, LOCAL_TIMEOUT).and_then(|o| if o.ok() { Ok(o) } else { Err(git_failure(&o.stderr, "record the merge")) })
        };
        with_index(&["read-tree", tree])?;
        for rel in conflicted {
            let entry = ask(self.top(), &["ls-tree", "-z", "--full-tree", "HEAD", "--", rel]).unwrap_or_default();
            match entry.trim_end_matches('\0').split_once('\t') {
                Some((meta, _)) => {
                    let f: Vec<&str> = meta.split(' ').collect();
                    let [mode, _, id] = f[..] else { return Err(fail(Kind::Git, "Couldn't read a note from HEAD")) };
                    with_index(&["update-index", "--add", "--cacheinfo", &format!("{mode},{id},{rel}")])?;
                }
                // Removed on this side: it stays removed.
                None => {
                    with_index(&["update-index", "--force-remove", "--", rel])?;
                }
            }
        }
        let tree = with_index(&["write-tree"])?.text();
        let msg = format!("Notes sync: kept my versions of {} (merged {})", plural(conflicted.len(), "note"), self.remote.upstream);
        Ok(self.must(self.top(), &["commit-tree", &tree, "-p", "HEAD", "-p", theirs, "-m", &msg], LOCAL_TIMEOUT, "record the merge")?.text())
    }

    fn push(&self) -> std::result::Result<(), Failure> {
        // Never forced: a plain refspec, and the remote branch this one follows.
        let refspec = format!("HEAD:{}", self.remote.merge);
        let out = self.run(&self.vault, &["push", "-q", "--recurse-submodules=no", &self.remote.name, &refspec], NET_TIMEOUT)?;
        if out.ok() { Ok(()) } else { Err(git_failure(&out.stderr, "push to the remote")) }
    }
}

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

/// A conflicted file's blobs: base, ours, theirs.
type Stages = [Option<String>; 3];

/// Removes `STARTED` once Git is done rewriting files.
struct Started(Option<PathBuf>);

impl Drop for Started {
    fn drop(&mut self) {
        if let Some(m) = &self.0 {
            let _ = std::fs::remove_file(m);
        }
    }
}

/// `text` with conflict-marker lines cut to the marker (their labels name
/// commits, which differ between Git's own merge and `git merge-file`).
fn without_labels(text: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    for line in text.split_inclusive(|&b| b == b'\n') {
        match [b"<<<<<<<".as_slice(), b"|||||||", b">>>>>>>"].iter().find(|m| line.starts_with(m)) {
            Some(m) => {
                out.extend_from_slice(m);
                out.push(b'\n');
            }
            None => out.extend_from_slice(line),
        }
    }
    out
}

/// Copy `path` to "name (conflict).ext" beside it (or "(conflict 2)"…), never replacing a file.
/// `taken` says a name is spoken for elsewhere (a file the abort will put back).
fn copy_aside(path: &Path, taken: &dyn Fn(&Path) -> bool) -> std::io::Result<PathBuf> {
    let bytes = std::fs::read(path)?;
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    for n in 1..1000 {
        let name = if n == 1 { format!("{stem} (conflict){ext}") } else { format!("{stem} (conflict {n}){ext}") };
        let copy = path.with_file_name(name);
        if taken(&copy) {
            continue;
        }
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&copy) {
            Ok(mut f) => {
                std::io::Write::write_all(&mut f, &bytes)?;
                f.sync_all()?;
                return Ok(copy);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("too many conflict copies"))
}

/// Vaults (false) and repositories (true) being synced in this process.
static ACTIVE: LazyLock<Mutex<HashSet<(bool, PathBuf)>>> = LazyLock::new(Mutex::default);

/// Holds a vault or repository in `ACTIVE` until dropped.
struct Claim((bool, PathBuf));

impl Claim {
    fn take(repo: bool, path: &Path) -> Option<Claim> {
        let key = (repo, std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned()));
        // (Not `then_some(Claim(..))`: a refused claim would be dropped, and
        // unlock, while the lock is still held.)
        let fresh = ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).insert(key.clone());
        if fresh { Some(Claim(key)) } else { None }
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).remove(&self.0);
    }
}

/// Sync `vault` now, on this thread. `phase` hears each step as it starts.
pub fn run(vault: &Path, opts: &Options, phase: &dyn Fn(Phase)) -> Outcome {
    let mut committed = 0;
    let result = run_inner(vault, opts, phase, &mut committed);
    let result = match result {
        Ok(r) => r,
        Err(f) => Result::Failed(f),
    };
    match &result {
        Result::Synced { committed, pulled, pushed } => log::info!("notes sync of {}: {committed} committed, {pulled} pulled, {pushed} pushed", vault.display()),
        Result::Conflict(c) => log::info!("notes sync of {}: {} in conflict with {}", vault.display(), plural(c.paths.len(), "file"), c.upstream),
        Result::Failed(f) => log::warn!("notes sync of {} failed ({:?}): {}{}", vault.display(), f.kind, f.message, f.detail.as_deref().map(|d| format!(" [{d}]")).unwrap_or_default()),
    }
    Outcome { committed, result }
}

fn run_inner(vault: &Path, opts: &Options, phase: &dyn Fn(Phase), committed: &mut usize) -> std::result::Result<Result, Failure> {
    let phase = |p: Phase| {
        phase(p);
        if let Some(f) = &opts.on_phase {
            f(p);
        }
    };
    phase(Phase::Check);
    let remote = describe(vault)?;
    let _claim = Claim::take(true, &remote.top).ok_or_else(|| fail(Kind::Busy, "Another sync is working in this repository"))?;
    let has_env = |k: &str| std::env::var_os(k).is_some() || opts.env.iter().any(|(e, _)| e == k);
    let batch_ssh = !has_env("GIT_SSH_COMMAND") && !has_env("GIT_SSH") && crate::git::config_value(vault, "core.sshCommand", None).is_none();
    // Only the repository sync was turned on for: checked before Git runs
    // anything from this one's config (status alone can start its fsmonitor).
    if let Some(expect) = &opts.expect
        && (expect.git_dir.is_empty() || identity_of(vault, &remote)? != *expect)
    {
        return Err(fail(Kind::Untrusted, "This isn't the repository Git sync was turned on for here (it was replaced, or its remote changed), so sync is off until you turn it on again. Nothing was run"));
    }
    let repo = Repo { vault: vault.to_owned(), remote, opts, batch_ssh };
    repo.recover()?;
    repo.check_clean()?;
    phase(Phase::Commit);
    *committed = repo.commit()?;
    let mut pulled = 0;
    // A push the remote turns away (it moved on meanwhile) gets one more round.
    for round in 0..2 {
        phase(Phase::Fetch);
        repo.fetch()?;
        let tracking = repo.remote.tracking.clone();
        let have_remote = repo.rev(&tracking).is_some();
        let (behind, ahead) = if have_remote {
            if ask(repo.top(), &["merge-base", "HEAD", &tracking]).is_none() {
                return Err(fail(Kind::Unrelated, format!("Your history and {}'s have nothing in common (was the remote replaced?). Nothing was pulled or pushed; sort it out in Git", repo.remote.upstream)));
            }
            (repo.count(&format!("HEAD..{tracking}")), repo.count(&format!("{tracking}..HEAD")))
        } else {
            // The remote branch doesn't exist yet: the push makes it.
            (0, repo.count("HEAD"))
        };
        if behind > 0 {
            repo.ignored_in_the_way(&tracking)?;
        }
        let conflict = match &opts.resolve {
            Some(r) if have_remote && behind > 0 => repo.settle(r, &phase)?,
            _ => repo.integrate(behind, ahead, &phase)?,
        };
        if let Some(c) = conflict {
            return Ok(Result::Conflict(c));
        }
        pulled += behind;
        let ahead = if have_remote { repo.count(&format!("{tracking}..HEAD")) } else { ahead };
        if ahead == 0 {
            return Ok(Result::Synced { committed: *committed, pulled, pushed: 0 });
        }
        phase(Phase::Push);
        match repo.push() {
            Ok(()) => return Ok(Result::Synced { committed: *committed, pulled, pushed: ahead }),
            Err(f) if f.kind == Kind::Rejected && round == 0 => continue,
            Err(f) => return Err(f),
        }
    }
    Err(fail(Kind::Rejected, "The remote kept changing while syncing; sync again"))
}

// ---- in the background ------------------------------------------------------------------

/// A sync running on its own thread.
pub struct Job {
    pub vault: PathBuf,
    /// Manual (the command) or the timer.
    pub manual: bool,
    rx: Receiver<Outcome>,
    /// Git is rewriting files in the working tree right now.
    integrating: Arc<AtomicBool>,
    /// Set when this job is let go of (see `Options::cancel`).
    cancel: Arc<AtomicBool>,
    /// The repository, once known (also in `HOLDS`).
    #[cfg(test)]
    top: Arc<Mutex<Option<PathBuf>>>,
}

impl Drop for Job {
    /// Quitting while Git rewrites files waits for it (past Git's own time
    /// limit), so a stopped rebase is undone rather than left half-done.
    fn drop(&mut self) {
        {
            // After this, the worker never starts rewriting files (see `Repo::enter`).
            let _gate = GATE.lock().unwrap_or_else(|e| e.into_inner());
            self.cancel.store(true, Ordering::SeqCst);
        }
        if self.integrating.load(Ordering::SeqCst) {
            let _ = self.rx.recv_timeout(LOCAL_TIMEOUT + Duration::from_secs(30));
        }
    }
}

/// Each running sync's "rewriting files" flag and repository, for every window.
type Hold = (Arc<AtomicBool>, Arc<Mutex<Option<PathBuf>>>);
static HOLDS: LazyLock<Mutex<Vec<Hold>>> = LazyLock::new(Mutex::default);

/// Is any sync (in any window) rewriting files of the repository `path` is in?
pub fn holds(path: &Path) -> bool {
    HOLDS.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|(flag, top)| holds_in(flag, top, path))
}

fn holds_in(flag: &AtomicBool, top: &Mutex<Option<PathBuf>>, path: &Path) -> bool {
    if !flag.load(Ordering::SeqCst) {
        return false;
    }
    let top = top.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let real = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_owned());
    let file = path.parent().map(|d| real(d).join(path.file_name().unwrap_or_default())).unwrap_or_else(|| path.to_owned());
    top.as_deref().is_none_or(|t| file.starts_with(real(t)))
}

impl Job {
    /// Start syncing `vault`; None when it's already being synced (in any window).
    pub fn start(vault: &Path, mut opts: Options, manual: bool, ctx: Option<egui::Context>) -> Option<Job> {
        let claim = Claim::take(false, vault)?;
        let cancel = Arc::new(AtomicBool::new(false));
        opts.cancel = Some(cancel.clone());
        let (tx, rx) = channel();
        let integrating = Arc::new(AtomicBool::new(false));
        let top = Arc::new(Mutex::new(None));
        let (flag, top_w, dir) = (integrating.clone(), top.clone(), vault.to_owned());
        let hold: Hold = (integrating.clone(), top.clone());
        HOLDS.lock().unwrap_or_else(|e| e.into_inner()).push(hold.clone());
        let unhold = move || HOLDS.lock().unwrap_or_else(|e| e.into_inner()).retain(|h| !Arc::ptr_eq(&h.0, &hold.0));
        std::thread::Builder::new()
            .name("notes-sync".into())
            .spawn(move || {
                let _claim = claim;
                let phase = |p: Phase| {
                    flag.store(p == Phase::Integrate, Ordering::SeqCst);
                    if p == Phase::Commit {
                        *top_w.lock().unwrap_or_else(|e| e.into_inner()) = ask(&dir, &["rev-parse", "--show-toplevel"]).map(PathBuf::from);
                    }
                };
                let outcome = run(&dir, &opts, &phase);
                flag.store(false, Ordering::SeqCst);
                unhold();
                let _ = tx.send(outcome);
                if let Some(ctx) = ctx {
                    ctx.request_repaint();
                }
            })
            .ok()?;
        Some(Job {
            vault: vault.to_owned(),
            manual,
            rx,
            integrating,
            cancel,
            #[cfg(test)]
            top,
        })
    }

    /// The outcome, once it's done.
    pub fn poll(&self) -> Option<Outcome> {
        self.rx.try_recv().ok()
    }

    /// Wait for the outcome (tests).
    #[cfg(test)]
    pub fn wait(&self) -> Outcome {
        self.rx.recv_timeout(Duration::from_secs(120)).expect("the sync finished")
    }

    /// Git may be rewriting `path` right now: don't save it until it's done.
    #[cfg(test)]
    pub fn holds(&self, path: &Path) -> bool {
        holds_in(&self.integrating, &self.top, path)
    }
}

#[cfg(test)]
#[path = "sync_tests.rs"]
pub(crate) mod repo_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocols_from_addresses() {
        for (url, p) in [
            ("https://github.com/me/notes.git", "https"),
            ("HTTPS://host/x", "https"),
            ("ssh://git@host:22/me/notes.git", "ssh"),
            ("git@github.com:me/notes.git", "ssh"),
            ("host:notes", "ssh"),
            ("git+ssh://host/x", "ssh"),
            ("file:///srv/notes.git", "file"),
            ("/srv/notes.git", "file"),
            ("../notes.git", "file"),
            ("C:\\Users\\me\\notes.git", "file"),
            ("C:/notes.git", "file"),
            ("./a:b", "file"),
        ] {
            assert_eq!(protocol_of(url), Ok(p), "{url}");
        }
        for url in ["http://host/x", "git://host/x", "ext::sh -c evil", "persistent-https::host/x", "ftp://host/x", "codecommit::us-east-1://repo"] {
            assert!(protocol_of(url).is_err(), "{url}");
        }
    }

    #[test]
    fn secrets_never_show() {
        // Every example is put together here, so no token-shaped text is
        // ever written out in the source (secret scanners would rightly object).
        let tail = |n: usize| -> String { "Ab1Cd2Ef3Gh4Ij5Kl6Mn7Op8Qr9St0UvWxYz".chars().cycle().take(n).collect() };
        let join = |parts: &[&str]| parts.concat();
        let bearer = join(&["eyJ", "hbGci", ".", &tail(12), ".sig"]);
        let basic = join(&["dXNl", "cjpw", "YXNz"]);
        let ghp = join(&["gh", "p_", &tail(36)]);
        let gho = join(&["gh", "o_", &tail(36)]);
        let pat = join(&["github", "_pat_", &tail(40)]);
        let glpat = join(&["glp", "at-", &tail(20)]);
        let aws = join(&["AK", "IA", &tail(16).to_uppercase()]);
        let openai = join(&["s", "k-proj-", &tail(32)]);
        let anthropic = join(&["sk", "-ant-", "api03-", &tail(30)]);
        let cases: Vec<(String, String)> = vec![
            (format!("remote: Authorization: Bearer {bearer}"), bearer.clone()),
            (format!("> authorization: Basic {basic}"), basic.clone()),
            (format!("hook: using bearer {bearer}"), bearer.clone()),
            ("fatal: https://h/x?token=s3cr3tvalue&x=1".into(), "s3cr3tvalue".into()),
            ("password=hunter2hunter2".into(), "hunter2hunter2".into()),
            ("CLIENT_SECRET: \"abcd1234efgh\"".into(), "abcd1234efgh".into()),
            ("api_key=AbC123xyz789".into(), "AbC123xyz789".into()),
            (format!("pushed with {ghp}"), ghp.clone()),
            (format!("{gho} expired"), gho.clone()),
            (format!("token {pat}"), pat.clone()),
            (format!("token {glpat}"), glpat.clone()),
            (format!("key {aws} leaked"), aws.clone()),
            (format!("OPENAI {openai}"), openai.clone()),
            (format!("ANTHROPIC {anthropic}"), anthropic.clone()),
        ];
        for (text, secret) in &cases {
            let (text, secret) = (text.as_str(), secret.as_str());
            let shown = redact(text);
            assert!(!shown.contains(secret), "{text} -> {shown}");
            assert!(shown.contains("[redacted]"), "{text} -> {shown}");
            assert!(!last_words(text).unwrap().contains(secret));
        }
        assert_eq!(redact("error: pre-commit hook said no"), "error: pre-commit hook said no", "ordinary text is untouched");
        assert_eq!(redact("Ask about tokens here: no secret"), "Ask about tokens here: no secret");
        assert!(last_words(&format!("fatal: {}", "x".repeat(2000))).unwrap().chars().count() <= 300, "capped");
    }

    #[test]
    fn passwords_never_show() {
        assert_eq!(redact("fatal: unable to access 'https://me:s3cret@host/x.git/': 403"), "fatal: unable to access 'https://host/x.git/': 403");
        assert_eq!(redact("ssh://git@host/x and https://tok@h/y"), "ssh://host/x and https://h/y");
        assert_eq!(redact("git@github.com:me/notes.git"), "git@github.com:me/notes.git", "scp-like addresses hold no password");
        assert_eq!(last_words("remote: x\nfatal: unable to access 'https://u:p@h/r/'\nhint: try\n").as_deref(), Some("fatal: unable to access 'https://h/r/'"));
    }

    #[test]
    fn errors_are_classified() {
        let cases = [
            ("fatal: could not read Username for 'https://h': terminal prompts disabled", Kind::Auth),
            ("git@h: Permission denied (publickey).\r\nfatal: Could not read from remote repository.", Kind::Auth),
            ("Host key verification failed.\nfatal: Could not read from remote repository.", Kind::Auth),
            ("ssh: Could not resolve hostname h: nodename nor servname provided\nfatal: Could not read from remote repository.", Kind::Offline),
            ("fatal: '/x/r.git' does not appear to be a git repository", Kind::Offline),
            ("fatal: unable to access 'https://h/': Failed to connect to h port 443", Kind::Offline),
            (" ! [rejected]        HEAD -> main (fetch first)\nerror: failed to push some refs", Kind::Rejected),
            ("Author identity unknown\n*** Please tell me who you are.", Kind::Identity),
            ("fatal: Unable to create '/r/.git/index.lock': File exists.", Kind::Busy),
            ("error: pre-commit hook said no", Kind::Git),
        ];
        for (text, kind) in cases {
            assert_eq!(classify(text), kind, "{text}");
        }
    }

    #[test]
    fn messages_fill_count_date_and_time() {
        let now = crate::notes::dates::DateTime { hour: 9, minute: 7, ..crate::notes::dates::DateTime::date(2026, 10, 19) };
        assert_eq!(fill_message("", 3, &now), "Notes sync: 3 changed, 2026-10-19 09:07");
        assert_eq!(fill_message("vault {date}: {count} files at {time} {other}", 1, &now), "vault 2026-10-19: 1 files at 09:07 {other}");
    }
}
