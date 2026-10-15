//! Copy Permalink: a GitHub, GitLab or Bitbucket link to the current line
//! (or lines) of a file at the commit checked out, made from the remote of
//! the file's repository. Git is only asked (read-only, with the hardening
//! flags in `git`), never configured.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Host {
    GitHub,
    GitLab,
    Bitbucket,
}

/// A hosted repository: `https://{host}/{path}`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Remote {
    pub kind: Host,
    /// Domain, with a port if the web server has one (from an http URL).
    pub host: String,
    /// "owner/repo" (GitLab groups may nest: "group/sub/repo").
    pub path: String,
}

/// A remote URL as git has it: `git@github.com:owner/repo.git`,
/// `ssh://git@host:2222/owner/repo`, `https://user:token@gitlab.com/g/r.git`,
/// `git://…`. None for local paths and hosts that aren't GitHub, GitLab or
/// Bitbucket (or self-hosted ones named after them).
pub fn parse_remote(url: &str) -> Option<Remote> {
    let url = url.trim();
    let (host, path, web_port) = if let Some((scheme, rest)) = url.split_once("://") {
        let scheme = scheme.to_ascii_lowercase();
        let (authority, path) = rest.split_once('/')?;
        // Credentials never go into a link.
        let authority = authority.rsplit('@').next()?;
        let (host, port) = match authority.rsplit_once(':') {
            Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) => (h, Some(p)),
            _ => (authority, None),
        };
        // An http(s) port is the web server's; an ssh port isn't.
        let web_port = if scheme.starts_with("http") { port } else { None };
        if !matches!(scheme.as_str(), "https" | "http" | "ssh" | "git" | "git+ssh" | "ssh+git") {
            return None;
        }
        (host.to_owned(), path.to_owned(), web_port)
    } else {
        // scp-like: [user@]host:path (a Windows drive letter or a local path isn't one).
        let (authority, path) = url.split_once(':')?;
        let host = authority.rsplit('@').next()?;
        if host.len() < 2 || host.contains('/') || host.contains('\\') || path.starts_with('\\') {
            return None;
        }
        (host.to_owned(), path.to_owned(), None)
    };
    let host = host.to_ascii_lowercase();
    // SSH over the HTTPS port: GitHub's and GitLab's alternate hosts.
    let host = match host.as_str() {
        "ssh.github.com" => "github.com".to_owned(),
        "altssh.gitlab.com" => "gitlab.com".to_owned(),
        "altssh.bitbucket.org" => "bitbucket.org".to_owned(),
        _ => host,
    };
    let kind = if host.contains("github") {
        Host::GitHub
    } else if host.contains("gitlab") {
        Host::GitLab
    } else if host.contains("bitbucket") {
        Host::Bitbucket
    } else {
        return None;
    };
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path).trim_end_matches('/');
    // GitLab's ssh paths may start with "~" for user namespaces; plain repos
    // need an owner and a name.
    if path.split('/').filter(|s| !s.is_empty()).count() < 2 || path.contains("..") {
        return None;
    }
    let host = match web_port {
        Some(p) => format!("{host}:{p}"),
        None => host,
    };
    Some(Remote { kind, host, path: path.to_owned() })
}

/// `s` percent-encoded for a URL path: everything but unreserved characters and `/`.
fn encode_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The web link to `file` (repo-relative, `/`-separated) at `commit`,
/// lines `first..=last` (1-based).
pub fn url(remote: &Remote, commit: &str, file: &str, first: usize, last: usize) -> String {
    let base = format!("https://{}/{}", remote.host, remote.path);
    let file = encode_path(file);
    let (first, last) = (first.min(last), first.max(last));
    match remote.kind {
        Host::GitHub if first == last => format!("{base}/blob/{commit}/{file}#L{first}"),
        Host::GitHub => format!("{base}/blob/{commit}/{file}#L{first}-L{last}"),
        Host::GitLab if first == last => format!("{base}/-/blob/{commit}/{file}#L{first}"),
        Host::GitLab => format!("{base}/-/blob/{commit}/{file}#L{first}-{last}"),
        Host::Bitbucket if first == last => format!("{base}/src/{commit}/{file}#lines-{first}"),
        Host::Bitbucket => format!("{base}/src/{commit}/{file}#lines-{first}:{last}"),
    }
}

/// Remotes by folder, and when git was asked.
type Cache = HashMap<PathBuf, (Instant, Option<Remote>)>;
static CACHE: Mutex<Option<Cache>> = Mutex::new(None);

/// The remote for the repository at `dir` (see `ask_remote`), cached for a
/// while per folder: menus ask every frame they're open.
pub fn remote_for(dir: &Path) -> Option<Remote> {
    const FRESH: Duration = Duration::from_secs(30);
    if let Some((at, remote)) = CACHE.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).get(dir)
        && at.elapsed() < FRESH
    {
        return remote.clone();
    }
    ask_remote(dir)
}

/// The current branch's upstream remote, else `origin`, else the first one
/// git lists, asked now (and cached for `remote_for`).
fn ask_remote(dir: &Path) -> Option<Remote> {
    use crate::git::git_line;
    let upstream = git_line(dir, &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"]);
    let mut names: Vec<String> = upstream.and_then(|u| u.split_once('/').map(|(r, _)| r.to_owned())).into_iter().collect();
    names.push("origin".into());
    names.extend(git_line(dir, &["remote"]).unwrap_or_default().lines().map(str::to_owned));
    let remote = names.iter().find_map(|name| git_line(dir, &["remote", "get-url", name]).as_deref().and_then(parse_remote));
    CACHE.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(dir.to_owned(), (Instant::now(), remote.clone()));
    remote
}

/// Why there's no permalink for a file, for the status bar.
pub const NO_REPO: &str = "Copy Permalink: the file isn't in a git repository with a commit";
pub const NO_REMOTE: &str = "Copy Permalink: the repository has no GitHub, GitLab or Bitbucket remote";

/// The permalink for lines `first..=last` (1-based) of `path`, asking git
/// afresh (a remote added a moment ago counts).
pub fn permalink(path: &Path, first: usize, last: usize) -> Result<String, &'static str> {
    use crate::git::git_line;
    let dir = crate::git::work_dir(path).ok_or(NO_REPO)?;
    let name = path.file_name().and_then(|n| n.to_str()).ok_or(NO_REPO)?;
    let commit = git_line(dir, &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"]).ok_or(NO_REPO)?;
    let remote = ask_remote(dir).ok_or(NO_REMOTE)?;
    // The folder's place in the repository ("" at its top).
    let prefix = git_line(dir, &["rev-parse", "--show-prefix"]).unwrap_or_default();
    Ok(url(&remote, &commit, &format!("{prefix}{name}"), first, last))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(kind: Host, host: &str, path: &str) -> Option<Remote> {
        Some(Remote { kind, host: host.into(), path: path.into() })
    }

    #[test]
    fn remote_formats() {
        let gh = r(Host::GitHub, "github.com", "owner/repo");
        assert_eq!(parse_remote("git@github.com:owner/repo.git"), gh);
        assert_eq!(parse_remote("git@github.com:owner/repo"), gh);
        assert_eq!(parse_remote("https://github.com/owner/repo.git"), gh);
        assert_eq!(parse_remote("https://github.com/owner/repo/"), gh);
        assert_eq!(parse_remote("https://user:s3cret@github.com/owner/repo.git"), gh, "credentials are dropped");
        assert_eq!(parse_remote("ssh://git@github.com/owner/repo.git"), gh);
        assert_eq!(parse_remote("ssh://git@ssh.github.com:443/owner/repo.git"), gh);
        assert_eq!(parse_remote("git://github.com/owner/repo.git"), gh);
        assert_eq!(parse_remote("  git@GitHub.com:owner/repo.git\n"), gh);
        assert_eq!(parse_remote("git@gitlab.com:group/sub/repo.git"), r(Host::GitLab, "gitlab.com", "group/sub/repo"));
        assert_eq!(parse_remote("ssh://git@gitlab.example.com:2222/team/repo.git"), r(Host::GitLab, "gitlab.example.com", "team/repo"), "an ssh port isn't the web's");
        assert_eq!(parse_remote("https://gitlab.example.com:8443/team/repo"), r(Host::GitLab, "gitlab.example.com:8443", "team/repo"));
        assert_eq!(parse_remote("git@bitbucket.org:team/repo.git"), r(Host::Bitbucket, "bitbucket.org", "team/repo"));
        assert_eq!(parse_remote("https://me@bitbucket.org/team/repo.git"), r(Host::Bitbucket, "bitbucket.org", "team/repo"));
        assert_eq!(parse_remote("git@github.example.com:org/repo.git"), r(Host::GitHub, "github.example.com", "org/repo"));
        // Not hosted repos.
        assert_eq!(parse_remote("/srv/git/repo.git"), None);
        assert_eq!(parse_remote("C:\\repos\\thing"), None);
        assert_eq!(parse_remote("file:///srv/git/repo.git"), None);
        assert_eq!(parse_remote("git@example.com:owner/repo.git"), None);
        assert_eq!(parse_remote("https://github.com/justowner"), None);
        assert_eq!(parse_remote(""), None);
    }

    #[test]
    fn link_formats() {
        let sha = "0123456789abcdef0123456789abcdef01234567";
        let gh = parse_remote("git@github.com:o/r.git").unwrap();
        assert_eq!(url(&gh, sha, "src/main.rs", 12, 12), format!("https://github.com/o/r/blob/{sha}/src/main.rs#L12"));
        assert_eq!(url(&gh, sha, "src/main.rs", 12, 20), format!("https://github.com/o/r/blob/{sha}/src/main.rs#L12-L20"));
        assert_eq!(url(&gh, sha, "a b/ü#.md", 20, 12), format!("https://github.com/o/r/blob/{sha}/a%20b/%C3%BC%23.md#L12-L20"));
        let gl = parse_remote("https://gitlab.com/g/s/r.git").unwrap();
        assert_eq!(url(&gl, sha, "x.py", 3, 4), format!("https://gitlab.com/g/s/r/-/blob/{sha}/x.py#L3-4"));
        assert_eq!(url(&gl, sha, "x.py", 3, 3), format!("https://gitlab.com/g/s/r/-/blob/{sha}/x.py#L3"));
        let bb = parse_remote("git@bitbucket.org:t/r.git").unwrap();
        assert_eq!(url(&bb, sha, "x.py", 3, 4), format!("https://bitbucket.org/t/r/src/{sha}/x.py#lines-3:4"));
        assert_eq!(url(&bb, sha, "x.py", 3, 3), format!("https://bitbucket.org/t/r/src/{sha}/x.py#lines-3"));
    }

    #[test]
    fn permalink_from_a_real_repository() {
        let dir = std::env::temp_dir().join(format!("strata-permalink-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub dir")).unwrap();
        let run = |args: &[&str]| std::process::Command::new("git").args(args).current_dir(&dir).output().is_ok_and(|o| o.status.success());
        if !run(&["init", "-q"]) {
            return; // no git here
        }
        let file = dir.join("sub dir/f.txt");
        std::fs::write(&file, "a\nb\n").unwrap();
        assert_eq!(permalink(&file, 1, 1), Err(NO_REPO), "no commit yet");
        assert!(run(&["add", "."]));
        assert!(run(&["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", "commit", "-q", "-m", "init"]));
        assert_eq!(permalink(&file, 1, 1), Err(NO_REMOTE));
        assert_eq!(remote_for(&dir.join("sub dir")), None, "cached");
        assert!(run(&["remote", "add", "upstream", "git@gitlab.com:team/proj.git"]));
        let sha = crate::git::git_line(&dir, &["rev-parse", "HEAD"]).unwrap();
        assert_eq!(permalink(&file, 1, 2), Ok(format!("https://gitlab.com/team/proj/-/blob/{sha}/sub%20dir/f.txt#L1-2")), "asked afresh");
        assert_eq!(remote_for(&dir.join("sub dir")), r(Host::GitLab, "gitlab.com", "team/proj"), "and cached");
        assert!(run(&["remote", "add", "origin", "https://github.com/me/proj.git"]));
        assert_eq!(permalink(&file, 2, 2), Ok(format!("https://github.com/me/proj/blob/{sha}/sub%20dir/f.txt#L2")), "origin is preferred");
        std::fs::remove_dir_all(dir).ok();
    }
}
