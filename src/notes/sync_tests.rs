//! Git sync against real repositories: a bare "remote" and two clones of
//! it ("here" and "there"), all in a temporary folder. Each test skips
//! itself when `git` isn't installed.

use super::*;
use std::collections::BTreeMap;

pub(crate) struct Fixture {
    pub base: PathBuf,
    pub remote: PathBuf,
    /// The clone Strata syncs; its vault is `here/vault`.
    pub here: PathBuf,
    /// Another device's clone.
    pub there: PathBuf,
    pub config: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

/// Git with no system or global config of the machine's.
pub(crate) fn git_env(config: &Path) -> Vec<(String, String)> {
    vec![("GIT_CONFIG_GLOBAL".into(), config.display().to_string()), ("GIT_CONFIG_NOSYSTEM".into(), "1".into())]
}

fn g_out(f: &Fixture, dir: &Path, args: &[&str]) -> std::process::Output {
    let mut cmd = Command::new("git");
    cmd.args(args).current_dir(dir).stdin(Stdio::null());
    for (k, v) in git_env(&f.config) {
        cmd.env(k, v);
    }
    cmd.output().expect("git runs")
}

/// Run git in `dir`; its output, asserting it worked.
pub(crate) fn g(f: &Fixture, dir: &Path, args: &[&str]) -> String {
    let out = g_out(f, dir, args);
    assert!(out.status.success(), "git {args:?} in {}: {}", dir.display(), String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

pub(crate) fn write(dir: &Path, rel: &str, text: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

pub(crate) fn read(dir: &Path, rel: &str) -> String {
    std::fs::read_to_string(dir.join(rel)).unwrap()
}

/// A remote with one commit (`vault/Plan.md`, `vault/Ideas.md`, `README.md`
/// outside the vault), cloned twice. None without git.
pub(crate) fn fixture(tag: &str) -> Option<Fixture> {
    Command::new("git").arg("--version").output().ok().filter(|o| o.status.success())?;
    let base = std::env::temp_dir().join(format!("strata-sync-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let base = std::fs::canonicalize(&base).unwrap();
    let config = base.join("gitconfig");
    std::fs::write(&config, "[user]\n\tname = Tester\n\temail = t@example.com\n[commit]\n\tgpgsign = false\n[init]\n\tdefaultBranch = main\n").unwrap();
    let f = Fixture { remote: base.join("remote.git"), here: base.join("here"), there: base.join("there"), config, base };
    g(&f, &f.base, &["init", "-q", "--bare", "-b", "main", "remote.git"]);
    let seed = f.base.join("seed");
    g(&f, &f.base, &["clone", "-q", f.remote.to_str().unwrap(), "seed"]);
    write(&seed, "vault/Plan.md", "# Plan\n\nline one\nline two\nline three\n");
    write(&seed, "vault/Ideas.md", "ideas\n");
    write(&seed, "README.md", "outside the vault\n");
    g(&f, &seed, &["add", "."]);
    g(&f, &seed, &["commit", "-q", "-m", "start"]);
    g(&f, &seed, &["push", "-q", "-u", "origin", "main"]);
    for d in ["here", "there"] {
        g(&f, &f.base, &["clone", "-q", f.remote.to_str().unwrap(), d]);
    }
    Some(f)
}

impl Fixture {
    pub(crate) fn vault(&self) -> PathBuf {
        self.here.join("vault")
    }

    fn opts(&self) -> Options {
        Options { env: git_env(&self.config), ..Options::default() }
    }

    fn sync(&self) -> Outcome {
        run(&self.vault(), &self.opts(), &|_| {})
    }

    /// Commit and push a change from the other clone.
    pub(crate) fn there_commits(&self, rel: &str, text: &str) {
        g(self, &self.there, &["pull", "-q", "--rebase"]);
        write(&self.there, rel, text);
        g(self, &self.there, &["add", "-A"]);
        g(self, &self.there, &["commit", "-q", "-m", "there"]);
        g(self, &self.there, &["push", "-q", "origin", "main"]);
    }

    /// A file as the remote's main has it.
    pub(crate) fn remote_text(&self, rel: &str) -> String {
        g(self, &self.remote, &["show", &format!("main:{rel}")])
    }

    pub(crate) fn remote_head(&self) -> String {
        g(self, &self.remote, &["rev-parse", "main"])
    }

    fn head(&self) -> String {
        g(self, &self.here, &["rev-parse", "HEAD"])
    }
}

/// Every file under `dir` (not `.git`) with its bytes.
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_owned()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if e.file_name() == ".git" {
                continue;
            }
            if p.is_dir() {
                stack.push(p);
            } else {
                out.insert(p.strip_prefix(dir).unwrap().to_owned(), std::fs::read(&p).unwrap());
            }
        }
    }
    out
}

fn synced(o: &Outcome) -> (usize, usize, usize) {
    match &o.result {
        Result::Synced { committed, pulled, pushed } => (*committed, *pulled, *pushed),
        other => panic!("not synced: {other:?}"),
    }
}

fn failed(o: &Outcome) -> &Failure {
    match &o.result {
        Result::Failed(f) => f,
        other => panic!("didn't fail: {other:?}"),
    }
}

#[test]
fn clean_sync_commits_and_pushes_then_nothing_to_do() {
    let Some(f) = fixture("clean") else { return };
    write(&f.vault(), "Plan.md", "# Plan\n\nline one\nline 2\nline three\n");
    write(&f.vault(), "New note.md", "new\n");
    let o = f.sync();
    assert_eq!(synced(&o), (2, 0, 1));
    assert_eq!(f.remote_text("vault/New note.md"), "new");
    assert_eq!(f.remote_head(), f.head());
    let msg = g(&f, &f.here, &["log", "-1", "--format=%s"]);
    assert!(msg.starts_with("Notes sync: 2 changed, 20"), "{msg}");
    // Nothing changed: nothing committed, nothing sent.
    let head = f.head();
    assert_eq!(synced(&f.sync()), (0, 0, 0));
    assert_eq!(f.head(), head);
    // A note deleted here is deleted on the remote too.
    std::fs::remove_file(f.vault().join("New note.md")).unwrap();
    assert_eq!(synced(&f.sync()), (1, 0, 1));
    assert!(!g(&f, &f.remote, &["ls-tree", "-r", "--name-only", "main"]).contains("New note.md"));
    // The message template is followed.
    write(&f.vault(), "Ideas.md", "more ideas\n");
    let o = run(&f.vault(), &Options { message: "vault: {count} file(s)".into(), ..f.opts() }, &|_| {});
    assert_eq!(synced(&o), (1, 0, 1));
    assert_eq!(g(&f, &f.here, &["log", "-1", "--format=%s"]), "vault: 1 file(s)");
}

#[test]
fn remote_ahead_is_pulled() {
    let Some(f) = fixture("pull") else { return };
    f.there_commits("vault/Ideas.md", "ideas from there\n");
    assert_eq!(synced(&f.sync()), (0, 1, 0));
    assert_eq!(read(&f.vault(), "Ideas.md"), "ideas from there\n");
    assert_eq!(f.head(), f.remote_head());
}

#[test]
fn both_ahead_without_conflict_rebases_into_a_straight_line() {
    let Some(f) = fixture("rebase") else { return };
    f.there_commits("vault/Ideas.md", "ideas from there\n");
    write(&f.vault(), "Plan.md", "# Plan\n\nline one\nline two\nline three\nline four\n");
    assert_eq!(synced(&f.sync()), (1, 1, 1));
    assert_eq!(read(&f.vault(), "Ideas.md"), "ideas from there\n");
    assert_eq!(f.remote_text("vault/Plan.md"), "# Plan\n\nline one\nline two\nline three\nline four");
    assert_eq!(g(&f, &f.here, &["rev-list", "--merges", "--count", "HEAD"]), "0", "rebased, not merged");
    assert_eq!(f.head(), f.remote_head());
}

#[test]
fn conflicting_edits_abort_the_rebase_and_keep_my_versions_settles_them() {
    let Some(f) = fixture("conflict") else { return };
    f.there_commits("vault/Plan.md", "# Plan\n\nline one\nTHEIR line two\nline three\n");
    write(&f.vault(), "Plan.md", "# Plan\n\nline one\nMY line two\nline three\n");
    write(&f.vault(), "Ideas.md", "my ideas\n");
    write(&f.here, "loose.txt", "untracked, outside the vault\n");
    let before = snapshot(&f.here);
    let remote_before = f.remote_head();
    let o = f.sync();
    let Result::Conflict(c) = &o.result else { panic!("{o:?}") };
    assert_eq!(o.committed, 2, "my edits are committed here");
    assert_eq!(c.paths, vec!["vault/Plan.md".to_owned()]);
    assert_eq!(c.upstream, "origin/main");
    assert_eq!(c.commit, remote_before);
    assert!(c.copies.is_empty());
    assert_eq!(snapshot(&f.here), before, "the working tree is byte for byte as it was");
    assert!(!f.base.join("here/.git/rebase-merge").exists() && !f.base.join("here/.git/rebase-apply").exists());
    assert_eq!(g(&f, &f.here, &["status", "--porcelain", "--untracked-files=no"]), "", "nothing half-done");
    assert_eq!(f.remote_head(), remote_before, "nothing pushed");
    // Syncing again without settling: the same conflict, nothing changes.
    let again = f.sync();
    assert!(matches!(again.result, Result::Conflict(_)), "{again:?}");
    assert_eq!(snapshot(&f.here), before);

    // Settling against a remote that changed the note again: look again.
    f.there_commits("vault/Plan.md", "# Plan\n\nline one\nTHEIR line two, edited\nline three\n");
    let resolve = Resolution { paths: c.paths.clone(), commit: c.commit.clone() };
    let o = run(&f.vault(), &Options { resolve: Some(resolve), ..f.opts() }, &|_| {});
    let Result::Conflict(moved) = &o.result else { panic!("{o:?}") };
    assert!(moved.moved);
    assert_eq!(moved.commit, f.remote_head());
    assert_eq!(snapshot(&f.here), before);

    // Merge by hand, then keep my versions: a merge commit with my file, pushed.
    let merged = "# Plan\n\nline one\nMY line two\nTHEIR line two, edited\nline three\n";
    write(&f.vault(), "Plan.md", merged);
    f.there_commits("vault/Other.md", "a new note from there\n");
    let resolve = Resolution { paths: moved.paths.clone(), commit: moved.commit.clone() };
    let o = run(&f.vault(), &Options { resolve: Some(resolve), ..f.opts() }, &|_| {});
    assert_eq!(synced(&o).0, 1);
    assert_eq!(read(&f.vault(), "Plan.md"), merged, "my text, untouched");
    assert_eq!(read(&f.vault(), "Other.md"), "a new note from there\n", "their other changes came in");
    assert_eq!(f.remote_text("vault/Plan.md"), merged.trim_end());
    assert_eq!(f.remote_text("vault/Ideas.md"), "my ideas");
    assert_eq!(f.head(), f.remote_head());
    assert_eq!(g(&f, &f.here, &["rev-list", "--merges", "--count", "HEAD"]), "1");
    assert!(g(&f, &f.here, &["log", "-1", "--format=%s"]).contains("kept my versions of 1 note"));
    assert_eq!(read(&f.here, "loose.txt"), "untracked, outside the vault\n");
    assert!(g(&f, &f.here, &["ls-files", "loose.txt"]).is_empty());
}

#[test]
fn a_rebase_that_stops_on_a_commit_in_between_is_undone() {
    let Some(f) = fixture("midway") else { return };
    // Here: one commit changes a line, the next changes it back. The two
    // ends merge cleanly with the remote's change, but replaying the first
    // commit doesn't: Git starts the rebase and stops.
    write(&f.vault(), "Plan.md", "# Plan\n\nline one\nX\nline three\n");
    g(&f, &f.here, &["commit", "-q", "-am", "x"]);
    write(&f.vault(), "Plan.md", "# Plan\n\nline one\nline two\nline three\n");
    g(&f, &f.here, &["commit", "-q", "-am", "back"]);
    let back = f.head();
    f.there_commits("vault/Plan.md", "# Plan\n\nline one\nTHEIRS\nline three\n");
    write(&f.vault(), "Ideas.md", "mine\n");
    let before = snapshot(&f.here);
    let o = f.sync();
    let Result::Conflict(c) = &o.result else { panic!("{o:?}") };
    assert_eq!(c.paths, vec!["vault/Plan.md".to_owned()]);
    assert_eq!(snapshot(&f.here), before, "byte for byte as it was");
    assert!(!f.here.join(".git/rebase-merge").exists());
    assert_eq!(g(&f, &f.here, &["rev-parse", "HEAD~1"]), back, "my commits sit where they were");
    assert_eq!(g(&f, &f.here, &["status", "--porcelain", "--untracked-files=no"]), "");
}

#[test]
fn an_unpushed_settling_merge_is_merged_again_not_replayed() {
    let Some(f) = fixture("merge-again") else { return };
    f.there_commits("vault/Plan.md", "theirs\n");
    write(&f.vault(), "Plan.md", "mine\n");
    let Result::Conflict(c) = f.sync().result else { panic!() };
    // Settle while pushes can't get through: the merge stays here, unpushed.
    let missing = f.base.join("missing.git");
    g(&f, &f.here, &["remote", "set-url", "--push", "origin", missing.to_str().unwrap()]);
    let o = run(&f.vault(), &Options { resolve: Some(Resolution { paths: c.paths.clone(), commit: c.commit.clone() }), ..f.opts() }, &|_| {});
    assert_eq!(failed(&o).kind, Kind::Offline, "{o:?}");
    assert_eq!(g(&f, &f.here, &["rev-list", "--merges", "--count", "origin/main..HEAD"]), "1");
    g(&f, &f.here, &["remote", "set-url", "--push", "origin", f.remote.to_str().unwrap()]);
    // Meanwhile the remote moves on. A rebase would replay "mine" onto
    // "theirs" and stop on the same conflict; a merge keeps the settlement.
    f.there_commits("vault/Ideas.md", "more from there\n");
    let o = f.sync();
    assert_eq!(synced(&o), (0, 1, 3), "my commit, the settling merge and the new merge");
    assert_eq!(read(&f.vault(), "Plan.md"), "mine\n");
    assert_eq!(read(&f.vault(), "Ideas.md"), "more from there\n");
    assert_eq!(f.remote_text("vault/Plan.md"), "mine");
    assert_eq!(f.head(), f.remote_head());
}

#[test]
fn no_upstream_changes_nothing() {
    let Some(f) = fixture("upstream") else { return };
    g(&f, &f.here, &["checkout", "-q", "-b", "local-only"]);
    write(&f.vault(), "Plan.md", "edited\n");
    let head = f.head();
    let o = f.sync();
    let e = failed(&o);
    assert_eq!(e.kind, Kind::NoUpstream);
    assert!(e.message.contains("“local-only” has no upstream"), "{}", e.message);
    assert_eq!((o.committed, f.head()), (0, head), "nothing committed");
    assert_eq!(read(&f.vault(), "Plan.md"), "edited\n");
    // Detached HEAD: the same.
    g(&f, &f.here, &["checkout", "-q", "--detach", "main"]);
    assert_eq!(failed(&f.sync()).kind, Kind::NoUpstream);
    // Not a repository at all.
    let plain = f.base.join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    std::fs::write(f.base.join("plain/n.md"), "x").unwrap();
    // (The temp folder might itself be inside a repository; only check the
    // answer when it isn't.)
    if ask(&plain, &["rev-parse", "--show-toplevel"]).is_none() {
        assert_eq!(failed(&run(&plain, &f.opts(), &|_| {})).kind, Kind::NotRepo);
    }
}

#[test]
fn unreachable_remote_keeps_the_local_commit() {
    let Some(f) = fixture("offline") else { return };
    let gone = format!("file://{}", f.base.join("gone.git").display());
    g(&f, &f.here, &["remote", "set-url", "origin", &gone]);
    write(&f.vault(), "Plan.md", "offline edit\n");
    let o = f.sync();
    let e = failed(&o);
    assert_eq!(e.kind, Kind::Offline, "{e:?}");
    assert_eq!(o.committed, 1, "committed here, for the next sync");
    assert_eq!(read(&f.vault(), "Plan.md"), "offline edit\n");
    // SSH to a host that doesn't resolve (an ssh that says so, so no network is used).
    #[cfg(unix)]
    {
        let ssh = f.base.join("fake-ssh");
        std::fs::write(&ssh, "#!/bin/sh\necho 'ssh: Could not resolve hostname notes.invalid: nodename nor servname provided, or not known' >&2\nexit 255\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o755)).unwrap();
        g(&f, &f.here, &["remote", "set-url", "origin", "git@notes.invalid:me/notes.git"]);
        g(&f, &f.here, &["config", "core.sshCommand", ssh.to_str().unwrap()]);
        let e = failed(&f.sync()).clone();
        assert_eq!(e.kind, Kind::Offline, "{e:?}");
        assert_eq!(e.detail.as_deref(), Some("ssh: Could not resolve hostname notes.invalid: nodename nor servname provided, or not known"));
    }
}

#[cfg(unix)]
#[test]
fn auth_failure_is_reported_and_nothing_prompts() {
    let Some(f) = fixture("auth") else { return };
    // An ssh that refuses the key, as a server without it would.
    let ssh = f.base.join("refusing-ssh");
    let log = f.base.join("ssh-args");
    std::fs::write(&ssh, format!("#!/bin/sh\necho \"$@\" > '{}'\nprintf 'git@notes.invalid: Permission denied (publickey).\\r\\n' >&2\nexit 255\n", log.display())).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o755)).unwrap();
    g(&f, &f.here, &["remote", "set-url", "origin", "ssh://git@notes.invalid/me/notes.git"]);
    let env = vec![("GIT_SSH_COMMAND".to_owned(), ssh.display().to_string())];
    let o = run(&f.vault(), &Options { env: [git_env(&f.config), env].concat(), ..Options::default() }, &|_| {});
    let e = failed(&o);
    assert_eq!(e.kind, Kind::Auth, "{e:?}");
    assert!(e.message.contains("refused the sign-in"));
    assert!(read(&f.base, "ssh-args").contains("notes.invalid"), "the user's own ssh command was used");
    // Without an ssh command of the user's, ssh runs in batch mode (never asks).
    let repo = Repo { vault: f.vault(), remote: describe(&f.vault()).unwrap(), opts: &f.opts(), batch_ssh: true };
    let cmd = repo.command(&f.vault(), &["fetch"]);
    let envs: Vec<_> = cmd.get_envs().map(|(k, v)| (k.to_string_lossy().into_owned(), v.map(|v| v.to_string_lossy().into_owned()))).collect();
    assert!(envs.contains(&("GIT_SSH_COMMAND".into(), Some("ssh -o BatchMode=yes".into()))));
    assert!(envs.contains(&("GIT_TERMINAL_PROMPT".into(), Some("0".into()))));
    // HTTPS with no stored credentials: Git would ask on a terminal; it may not.
    let o = Out { code: Some(128), stdout: Vec::new(), stderr: "fatal: could not read Username for 'https://notes.invalid': terminal prompts disabled".into() };
    assert_eq!(git_failure(&o.stderr, "fetch").kind, Kind::Auth);
}

#[test]
fn only_the_remotes_protocol_is_allowed() {
    let Some(f) = fixture("protocol") else { return };
    // The hardened read-only Git can't reach even a local remote...
    let read_only = crate::git::git_io_code(&f.here, &["fetch", "-q", "origin"], None, 1 << 20, READ_TIMEOUT);
    assert!(read_only.is_some_and(|(code, _)| code != Some(0)), "protocol.file is off for scans");
    // ...the sync's own command allows exactly the remote's protocol.
    let remote = describe(&f.vault()).unwrap();
    assert_eq!(remote.protocols, vec!["file"]);
    assert_eq!((remote.upstream.as_str(), remote.merge.as_str(), remote.prefix.as_str()), ("origin/main", "refs/heads/main", "vault/"));
    f.there_commits("vault/Ideas.md", "from there\n");
    assert_eq!(synced(&f.sync()), (0, 1, 0), "fetched over file://");
    // A remote that runs a program (ext::), directly or through insteadOf, is refused.
    g(&f, &f.here, &["config", "protocol.ext.allow", "always"]);
    g(&f, &f.here, &["config", "url.ext::sh -c touch% /tmp/strata-pwned% #.insteadOf", f.remote.to_str().unwrap()]);
    let e = failed(&f.sync()).clone();
    assert_eq!(e.kind, Kind::Unsupported, "{e:?}");
    g(&f, &f.here, &["config", "--remove-section", "url.ext::sh -c touch% /tmp/strata-pwned% #"]);
    g(&f, &f.here, &["remote", "set-url", "--push", "origin", "http://notes.invalid/x.git"]);
    assert_eq!(failed(&f.sync()).kind, Kind::Unsupported, "a plain-http push address too");
}

#[test]
fn files_outside_the_vault_are_never_committed_or_touched() {
    let Some(f) = fixture("outside") else { return };
    write(&f.here, "loose.txt", "untracked outside\n");
    write(&f.vault(), "Plan.md", "inside\n");
    assert_eq!(synced(&f.sync()), (1, 0, 1));
    assert!(g(&f, &f.here, &["ls-files", "loose.txt"]).is_empty(), "not committed");
    assert_eq!(read(&f.here, "loose.txt"), "untracked outside\n");
    // A tracked file changed outside the vault would stop the rebase: refuse up front.
    write(&f.here, "README.md", "changed outside\n");
    g(&f, &f.here, &["add", "README.md"]);
    write(&f.vault(), "Ideas.md", "inside again\n");
    let head = f.head();
    let o = f.sync();
    let e = failed(&o);
    assert_eq!(e.kind, Kind::OutsideChanges);
    assert!(e.message.contains("README.md"), "{}", e.message);
    assert_eq!((o.committed, f.head()), (0, head));
    assert_eq!(read(&f.here, "README.md"), "changed outside\n");
    assert_eq!(g(&f, &f.here, &["diff", "--cached", "--name-only"]), "README.md", "still staged, as the user left it");
    assert_eq!(read(&f.vault(), "Ideas.md"), "inside again\n");
}

#[test]
fn ignored_files_are_not_committed() {
    let Some(f) = fixture("ignored") else { return };
    write(&f.vault(), ".gitignore", "*.tmp\nprivate/\n");
    write(&f.vault(), "scratch.tmp", "temp\n");
    write(&f.vault(), "private/diary.md", "secret\n");
    write(&f.vault(), "Kept.md", "kept\n");
    assert_eq!(synced(&f.sync()), (2, 0, 1), ".gitignore and Kept.md");
    let files = g(&f, &f.remote, &["ls-tree", "-r", "--name-only", "main"]);
    assert!(files.contains("vault/Kept.md") && files.contains("vault/.gitignore"));
    assert!(!files.contains("scratch.tmp") && !files.contains("diary"), "{files}");
    assert_eq!(read(&f.vault(), "private/diary.md"), "secret\n");
}

#[cfg(unix)]
#[test]
fn hooks_run_and_a_refusing_hook_stops_the_sync() {
    use std::os::unix::fs::PermissionsExt;
    let Some(f) = fixture("hooks") else { return };
    let hook = f.here.join(".git/hooks/pre-commit");
    let marker = f.base.join("hook-ran");
    std::fs::write(&hook, format!("#!/bin/sh\ntouch '{}'\nexit 0\n", marker.display())).unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    write(&f.vault(), "Plan.md", "one\n");
    assert_eq!(synced(&f.sync()), (1, 0, 1));
    assert!(marker.exists(), "the repository's hooks run (sync was turned on knowing that)");
    std::fs::write(&hook, "#!/bin/sh\necho 'pre-commit: no' >&2\nexit 1\n").unwrap();
    write(&f.vault(), "Plan.md", "two\n");
    let remote = f.remote_head();
    let o = f.sync();
    let e = failed(&o);
    assert_eq!(e.kind, Kind::Git);
    assert!(e.message.contains("hook may have refused"), "{}", e.message);
    assert_eq!(e.detail.as_deref(), Some("pre-commit: no"));
    assert_eq!(read(&f.vault(), "Plan.md"), "two\n");
    assert_eq!(f.remote_head(), remote, "nothing pushed");
}

#[cfg(unix)]
#[test]
fn a_file_written_while_git_is_stopped_is_kept_as_a_conflict_copy() {
    use std::os::unix::fs::PermissionsExt;
    let Some(f) = fixture("stray") else { return };
    f.there_commits("vault/Ideas.md", "theirs\n");
    write(&f.vault(), "Plan.md", "mine\n");
    // Another program (Obsidian, say) writes Plan.md just as the rebase
    // begins: a hook Git runs then stands in for it (once). Replaying my
    // commit would overwrite it, so Git stops; undoing the rebase puts my
    // committed text back, so the other program's is kept aside first.
    let hook = f.here.join(".git/hooks/post-checkout");
    let once = f.base.join("hook-done");
    let script = format!(
        "#!/bin/sh\n[ -d \"$(git rev-parse --git-path rebase-merge)\" ] || exit 0\n[ -e '{}' ] && exit 0\ntouch '{}'\nprintf 'written meanwhile\\n' > '{}'\n",
        once.display(),
        once.display(),
        f.vault().join("Plan.md").display()
    );
    std::fs::write(&hook, script).unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    let head = f.head();
    let o = f.sync();
    let e = failed(&o);
    assert_eq!(e.kind, Kind::Git, "{o:?}");
    assert!(once.exists(), "the stand-in ran mid-rebase");
    assert!(e.message.contains("your files are as they were. Text another program wrote meanwhile was kept as Plan (conflict).md"), "{}", e.message);
    assert_eq!(read(&f.vault(), "Plan (conflict).md"), "written meanwhile\n", "the other program's text is kept");
    assert_eq!(read(&f.vault(), "Plan.md"), "mine\n");
    assert_eq!(read(&f.vault(), "Ideas.md"), "ideas\n", "nothing of the remote's came in");
    assert_eq!(g(&f, &f.here, &["rev-parse", "HEAD~1"]), head, "the rebase was undone: my commit sits where it was");
    assert!(!f.here.join(".git/rebase-merge").exists());
    assert_eq!(crate::notes::conflicts::original_name("Plan (conflict).md").as_deref(), Some("Plan.md"), "Notes shows it as a conflict copy");
}

#[test]
fn unrelated_history_changes_nothing() {
    let Some(f) = fixture("unrelated") else { return };
    // The remote is replaced by a history of its own.
    let other = f.base.join("other");
    g(&f, &f.base, &["init", "-q", "-b", "main", "other"]);
    write(&other, "vault/Plan.md", "a different history\n");
    g(&f, &other, &["add", "."]);
    g(&f, &other, &["commit", "-q", "-m", "other"]);
    g(&f, &other, &["push", "-q", "--force", f.remote.to_str().unwrap(), "main"]);
    write(&f.vault(), "Plan.md", "mine\n");
    let remote = f.remote_head();
    let o = f.sync();
    assert_eq!(failed(&o).kind, Kind::Unrelated);
    assert_eq!(f.remote_head(), remote, "never forced");
    assert_eq!(read(&f.vault(), "Plan.md"), "mine\n");
}

#[test]
fn a_remote_branch_that_doesnt_exist_yet_is_made_by_the_push() {
    let Some(f) = fixture("new-branch") else { return };
    g(&f, &f.here, &["checkout", "-q", "-b", "laptop"]);
    g(&f, &f.here, &["config", "branch.laptop.remote", "origin"]);
    g(&f, &f.here, &["config", "branch.laptop.merge", "refs/heads/laptop"]);
    write(&f.vault(), "Plan.md", "on the laptop branch\n");
    assert_eq!(synced(&f.sync()).0, 1);
    assert_eq!(g(&f, &f.remote, &["show", "laptop:vault/Plan.md"]), "on the laptop branch");
}

#[test]
fn syncs_never_overlap() {
    let Some(f) = fixture("overlap") else { return };
    let (go_tx, go_rx) = std::sync::mpsc::sync_channel::<()>(0);
    let (at_tx, at_rx) = std::sync::mpsc::sync_channel::<Phase>(8);
    let go_rx = Mutex::new(go_rx);
    let on_phase: Arc<dyn Fn(Phase) + Send + Sync> = Arc::new(move |p| {
        let _ = at_tx.send(p);
        if p == Phase::Fetch {
            let _ = go_rx.lock().unwrap().recv();
        }
    });
    write(&f.vault(), "Plan.md", "edited\n");
    let job = Job::start(&f.vault(), Options { on_phase: Some(on_phase), ..f.opts() }, false, None).expect("starts");
    while at_rx.recv().unwrap() != Phase::Fetch {}
    // While it's held at Fetch: no second sync of this vault, nor of its repository from elsewhere.
    assert!(Job::start(&f.vault(), f.opts(), true, None).is_none());
    let o = run(&f.here, &f.opts(), &|_| {});
    assert_eq!(failed(&o).kind, Kind::Busy, "{o:?}");
    go_tx.send(()).unwrap();
    assert_eq!(synced(&job.wait()), (1, 0, 1));
    // Done: it can run again.
    let again = Job::start(&f.vault(), f.opts(), true, None).expect("starts once the first is done");
    assert_eq!(synced(&again.wait()), (0, 0, 0));
}

#[test]
fn saves_wait_while_git_rewrites_files() {
    let Some(f) = fixture("holds") else { return };
    f.there_commits("vault/Ideas.md", "from there\n");
    let (seen_tx, seen_rx) = std::sync::mpsc::channel::<bool>();
    let holder: Arc<Mutex<Option<Arc<AtomicBool>>>> = Arc::default();
    let h2 = holder.clone();
    let on_phase: Arc<dyn Fn(Phase) + Send + Sync> = Arc::new(move |p| {
        if p == Phase::Integrate {
            // Called just after the job raised its flag.
            let _ = seen_tx.send(h2.lock().unwrap().as_ref().is_some_and(|f| f.load(Ordering::SeqCst)));
        }
    });
    let job = Job::start(&f.vault(), Options { on_phase: Some(on_phase), ..f.opts() }, true, None).unwrap();
    *holder.lock().unwrap() = Some(job.integrating.clone());
    let _ = job.wait();
    assert!(seen_rx.try_recv().unwrap_or(true), "the flag is up while Git integrates");
    assert!(!job.holds(&f.vault().join("Plan.md")), "and down once it's done");
    job.integrating.store(true, Ordering::SeqCst);
    assert!(job.holds(&f.vault().join("Plan.md")));
    assert!(job.holds(&f.here.join("README.md")), "anywhere in the repository");
    assert!(!job.holds(&f.base.join("elsewhere.md")));
}

#[test]
fn remote_files_this_computer_ignores_are_never_overwritten() {
    let Some(f) = fixture("ignored-incoming") else { return };
    // Ignored here (not in .gitignore, so the other device commits it).
    std::fs::write(f.here.join(".git/info/exclude"), "vault/drafts/\n").unwrap();
    write(&f.vault(), "drafts/idea.md", "my local draft\n");
    f.there_commits("vault/drafts/idea.md", "their draft\n");
    write(&f.vault(), "Plan.md", "mine\n");
    let remote = f.remote_head();
    let o = f.sync();
    let e = failed(&o);
    assert_eq!(e.kind, Kind::OutsideChanges, "{e:?}");
    assert!(e.message.contains("would replace files this computer ignores (vault/drafts/idea.md)"), "{}", e.message);
    assert_eq!(read(&f.vault(), "drafts/idea.md"), "my local draft\n");
    assert_eq!(f.remote_head(), remote);
}

#[cfg(unix)]
#[test]
fn a_rebase_stopped_for_taking_too_long_is_undone_with_its_hooks() {
    use std::os::unix::fs::PermissionsExt;
    let Some(f) = fixture("slow") else { return };
    f.there_commits("vault/Ideas.md", "theirs\n");
    write(&f.vault(), "Plan.md", "mine\n");
    // A hook that hangs as the rebase checks out the remote's commit (a slow
    // LFS download, say; only then, not when the rebase is undone), and would
    // write a file late if it outlived the sync.
    let (once, late) = (f.base.join("hook-done"), f.base.join("late"));
    let hook = f.here.join(".git/hooks/post-checkout");
    let script = format!(
        "#!/bin/sh\n[ \"$2\" = '{2}' ] || exit 0\n[ -e '{0}' ] && exit 0\nps -o pid,pgid,ppid -p $PPID > '{0}'\nsleep 15\ntouch '{1}'\n",
        once.display(),
        late.display(),
        f.remote_head()
    );
    std::fs::write(&hook, script).unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    let head = f.head();
    let started = Instant::now();
    let o = run(&f.vault(), &Options { integrate_timeout: Some(Duration::from_secs(6)), ..f.opts() }, &|_| {});
    assert!(once.exists(), "the hook ran");
    let e = failed(&o);
    assert!(matches!(e.kind, Kind::Offline | Kind::Git | Kind::Stuck), "{e:?}");
    assert!(!f.here.join(".git/rebase-merge").exists(), "the rebase was undone: {e:?}");
    assert_eq!(g(&f, &f.here, &["rev-parse", "HEAD~1"]), head);
    assert_eq!(g(&f, &f.here, &["symbolic-ref", "--short", "HEAD"]), "main", "on the branch, not detached");
    assert_eq!(read(&f.vault(), "Plan.md"), "mine\n");
    assert_eq!(read(&f.vault(), "Ideas.md"), "ideas\n");
    std::thread::sleep(Duration::from_secs(16).saturating_sub(started.elapsed()) + Duration::from_millis(500));
    assert!(!late.exists(), "the hook was stopped along with Git: {e:?} after {:?}; hook: {}", started.elapsed(), std::fs::read_to_string(&once).unwrap_or_default());
}

#[test]
fn nothing_asks_for_a_password() {
    let Some(f) = fixture("askpass") else { return };
    let repo = Repo { vault: f.vault(), remote: describe(&f.vault()).unwrap(), opts: &f.opts(), batch_ssh: false };
    let cmd = repo.command(&f.vault(), &["fetch"]);
    let envs: Vec<_> = cmd.get_envs().map(|(k, v)| (k.to_string_lossy().into_owned(), v.map(|v| v.to_string_lossy().into_owned()))).collect();
    for (k, v) in [("GIT_ASKPASS", ""), ("SSH_ASKPASS", ""), ("GIT_TERMINAL_PROMPT", "0"), ("GCM_INTERACTIVE", "never")] {
        assert!(envs.contains(&(k.into(), Some(v.into()))), "{k}");
    }
    assert!(envs.contains(&("GIT_DIR".into(), None)), "an inherited GIT_DIR is dropped");
}

/// The "midway" history: one commit changes a line and the next changes it
/// back, so the ends merge cleanly but replaying the first stops on Plan.md.
pub(crate) fn midway(f: &Fixture) {
    write(&f.vault(), "Plan.md", "# Plan\n\nline one\nX\nline three\n");
    g(f, &f.here, &["commit", "-q", "-am", "x"]);
    write(&f.vault(), "Plan.md", "# Plan\n\nline one\nline two\nline three\n");
    g(f, &f.here, &["commit", "-q", "-am", "back"]);
    f.there_commits("vault/Plan.md", "# Plan\n\nline one\nTHEIRS\nline three\n");
}

#[test]
fn text_saved_into_a_conflicted_file_mid_rebase_is_kept() {
    let Some(f) = fixture("stray-unmerged") else { return };
    midway(&f);
    // An older conflict copy is never replaced: the new one is numbered.
    write(&f.vault(), "Plan (conflict).md", "an older copy\n");
    let plan = f.vault().join("Plan.md");
    let saved: Arc<dyn Fn() + Send + Sync> = Arc::new(move || std::fs::write(&plan, "saved by another program meanwhile\n").unwrap());
    let o = run(&f.vault(), &Options { before_abort: Some(saved), ..f.opts() }, &|_| {});
    let Result::Conflict(c) = &o.result else { panic!("{o:?}") };
    assert_eq!(c.paths, vec!["vault/Plan.md".to_owned()]);
    assert_eq!(c.copies, vec![f.vault().join("Plan (conflict 2).md")]);
    assert_eq!(read(&f.vault(), "Plan (conflict 2).md"), "saved by another program meanwhile\n");
    assert_eq!(read(&f.vault(), "Plan (conflict).md"), "an older copy\n");
    assert_eq!(read(&f.vault(), "Plan.md"), "# Plan\n\nline one\nline two\nline three\n", "the committed text is back");
    // Git's own conflict output (markers and all) isn't copied.
    let o = f.sync();
    let Result::Conflict(c) = &o.result else { panic!("{o:?}") };
    assert!(c.copies.is_empty(), "{:?}", c.copies);
}

#[test]
fn a_different_repository_at_the_same_path_is_refused_before_git_runs() {
    let Some(f) = fixture("identity") else { return };
    let first = identity(&f.vault()).expect("an identity");
    assert!(!first.git_dir.is_empty() && !first.root.is_empty() && !first.remote.is_empty(), "{first:?}");
    assert_eq!(identity(&f.vault()).unwrap(), first, "stable");
    let expect = |i: &Identity| Options { expect: Some(i.clone()), ..f.opts() };
    write(&f.vault(), "Plan.md", "edited\n");
    assert_eq!(synced(&run(&f.vault(), &expect(&first), &|_| {})).0, 1, "the same repository syncs");
    // An enablement without an identity (typed into settings.json) is asked about too.
    assert_eq!(failed(&run(&f.vault(), &expect(&Identity::default()), &|_| {})).kind, Kind::Untrusted);
    // Another repository at the same path, with a hook and its own remote.
    std::fs::remove_dir_all(&f.here).unwrap();
    g(&f, &f.base, &["init", "-q", "--bare", "-b", "main", "other-remote.git"]);
    g(&f, &f.base, &["clone", "-q", f.base.join("other-remote.git").to_str().unwrap(), "here"]);
    write(&f.vault(), "Plan.md", "someone else's repository\n");
    g(&f, &f.here, &["add", "."]);
    g(&f, &f.here, &["commit", "-q", "-m", "theirs"]);
    g(&f, &f.here, &["push", "-q", "-u", "origin", "main"]);
    let marker = f.base.join("hook-ran");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let hook = f.here.join(".git/hooks/pre-commit");
        std::fs::write(&hook, format!("#!/bin/sh\ntouch '{}'\n", marker.display())).unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    write(&f.vault(), "Plan.md", "changed\n");
    let head = f.head();
    let spawned_before = git_spawns();
    let o = run(&f.vault(), &expect(&first), &|_| {});
    let e = failed(&o);
    assert_eq!(e.kind, Kind::Untrusted, "{e:?}");
    assert_eq!(git_spawns(), spawned_before, "refused before any Git process ran in it");
    assert!(!marker.exists(), "nothing of the other repository's ran");
    assert_eq!((o.committed, f.head()), (0, head));
    assert_ne!(identity(&f.vault()).unwrap(), first);
}

#[test]
fn a_sync_stopped_before_integrating_never_starts_a_rebase() {
    let Some(f) = fixture("cancel") else { return };
    f.there_commits("vault/Ideas.md", "theirs\n");
    write(&f.vault(), "Plan.md", "mine\n");
    let (go_tx, go_rx) = std::sync::mpsc::sync_channel::<()>(0);
    let (at_tx, at_rx) = std::sync::mpsc::sync_channel::<Phase>(8);
    let go_rx = Mutex::new(go_rx);
    let on_phase: Arc<dyn Fn(Phase) + Send + Sync> = Arc::new(move |p| {
        let _ = at_tx.send(p);
        if p == Phase::Fetch {
            let _ = go_rx.lock().unwrap().recv();
        }
    });
    let job = Job::start(&f.vault(), Options { on_phase: Some(on_phase), ..f.opts() }, false, None).unwrap();
    while at_rx.recv().unwrap() != Phase::Fetch {}
    let head = f.head();
    // Quitting (the window lets go of the job) while it fetches.
    drop(job);
    go_tx.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    while busy(&f.vault()) {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(at_rx.try_iter().all(|p| p != Phase::Integrate), "never began rewriting files");
    assert_eq!(f.head(), head, "no rebase");
    assert_eq!(read(&f.vault(), "Ideas.md"), "ideas\n");
    assert!(!f.here.join(".git/rebase-merge").exists());
}

/// A process id that has ended (its start time can't match anything).
fn gone_pid() -> u32 {
    let mut c = Command::new(if cfg!(windows) { "cmd" } else { "true" });
    if cfg!(windows) {
        c.args(["/C", "exit"]);
    }
    let mut child = c.spawn().expect("a short process");
    let pid = child.id();
    child.wait().unwrap();
    pid
}

/// The midway history, fetched, with a rebase onto the remote stopped on
/// Plan.md (as Git leaves it); returns (HEAD before, the commit it went onto).
fn stopped_rebase(f: &Fixture) -> (String, String) {
    midway(f);
    g(f, &f.here, &["fetch", "-q"]);
    let (head, onto) = (f.head(), g(f, &f.here, &["rev-parse", "origin/main"]));
    let out = g_out(f, &f.here, &["rebase", "-q", "origin/main"]);
    assert!(!out.status.success() && f.here.join(".git/rebase-merge").exists());
    (head, onto)
}

#[test]
fn a_rebase_left_behind_by_a_quit_is_undone_on_the_next_sync() {
    let Some(f) = fixture("leftover") else { return };
    midway(&f);
    g(&f, &f.here, &["fetch", "-q"]);
    let (head, onto) = (f.head(), g(&f, &f.here, &["rev-parse", "origin/main"]));
    // As a sync would have marked it, from a Strata that has since quit.
    write_marker(&f.vault(), "rebase", gone_pid(), 1, &head, "main", &onto, true);
    let out = g_out(&f, &f.here, &["rebase", "-q", "origin/main"]);
    assert!(!out.status.success() && f.here.join(".git/rebase-merge").exists());
    let o = f.sync();
    let e = failed(&o);
    assert!(e.message.contains("A sync that was stopped when Strata quit has been undone"), "{e:?}");
    assert!(!f.here.join(".git/rebase-merge").exists() && !f.here.join(".git").join(STARTED).exists());
    assert_eq!(read(&f.vault(), "Plan.md"), "# Plan\n\nline one\nline two\nline three\n");
    assert_eq!(g(&f, &f.here, &["symbolic-ref", "--short", "HEAD"]), "main");
    // A rebase someone started by hand, with no marker at all, is theirs.
    let out = g_out(&f, &f.here, &["rebase", "-q", "origin/main"]);
    assert!(!out.status.success());
    assert_eq!(failed(&f.sync()).kind, Kind::Busy);
    assert!(f.here.join(".git/rebase-merge").exists());
}

/// The marker doesn't match, isn't sealed, or its Strata still runs: the
/// stopped rebase (and any resolutions in it) is left exactly as it is.
fn assert_left_alone(f: &Fixture) {
    let before = snapshot(&f.here);
    let o = f.sync();
    let e = failed(&o);
    assert_eq!(e.kind, Kind::Busy, "{e:?}");
    assert!(e.message.contains("An unfinished Git operation"), "{e:?}");
    assert!(f.here.join(".git/rebase-merge").exists(), "not aborted");
    assert!(f.here.join(".git").join(STARTED).exists(), "the marker is kept");
    assert_eq!(snapshot(&f.here), before, "the user's state, byte for byte");
}

#[test]
fn a_stale_marker_never_undoes_a_rebase_started_by_hand() {
    let Some(f) = fixture("stale-marker") else { return };
    midway(&f);
    g(&f, &f.here, &["fetch", "-q"]);
    // A marker from an older sync (a crash), onto an older remote commit…
    let (head, old_onto) = (f.head(), g(&f, &f.here, &["rev-parse", "origin/main"]));
    write_marker(&f.vault(), "rebase", gone_pid(), 1, &head, "main", &old_onto, true);
    // …then the user rebases onto newer work by hand, and resolves part of it.
    f.there_commits("vault/Ideas.md", "newer\n");
    g(&f, &f.here, &["fetch", "-q"]);
    let out = g_out(&f, &f.here, &["rebase", "-q", "origin/main"]);
    assert!(!out.status.success());
    write(&f.vault(), "Plan.md", "my resolution in progress\n");
    assert_left_alone(&f);
    assert_eq!(read(&f.vault(), "Plan.md"), "my resolution in progress\n");
}

#[test]
fn a_planted_marker_is_ignored() {
    let Some(f) = fixture("planted-marker") else { return };
    let (head, onto) = stopped_rebase(&f);
    // Every field right, but not sealed with this installation's key.
    write_marker(&f.vault(), "rebase", gone_pid(), 1, &head, "main", &onto, false);
    assert_left_alone(&f);
}

#[test]
fn a_marker_of_a_strata_still_running_is_left_alone() {
    let Some(f) = fixture("live-marker") else { return };
    let (head, onto) = stopped_rebase(&f);
    let me = std::process::id();
    write_marker(&f.vault(), "rebase", me, process_start(me).expect("our own start time"), &head, "main", &onto, true);
    assert_left_alone(&f);
}

#[test]
fn keeping_files_aside_fails_closed() {
    for fault in ["list", "read", "create", "names"] {
        let Some(f) = fixture(&format!("fault-{fault}")) else { return };
        midway(&f);
        let plan = f.vault().join("Plan.md");
        let saved: Arc<dyn Fn() + Send + Sync> = Arc::new(move || std::fs::write(&plan, "saved by another program meanwhile\n").unwrap());
        let o = run(&f.vault(), &Options { before_abort: Some(saved), preserve_fault: Some(fault), ..f.opts() }, &|_| {});
        let e = failed(&o);
        assert_eq!(e.kind, Kind::Stuck, "{fault}: {e:?}");
        assert!(e.message.contains("couldn't keep a copy") && e.message.contains("nothing was undone"), "{fault}: {}", e.message);
        assert!(f.here.join(".git/rebase-merge").exists(), "{fault}: left stopped, not aborted");
        assert_eq!(read(&f.vault(), "Plan.md"), "saved by another program meanwhile\n", "{fault}: the text is still there");
    }
}
