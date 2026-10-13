//! Command line: `strata [options] [file[:line[:col]] | folder]...`

use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub const USAGE: &str = "\
Usage: strata [options] [file[:line[:column]] | folder]...

Opens each file in a tab (a file that doesn't exist yet opens empty and is
created on first save) and each folder in the sidebar. If Strata is already
running, the paths open in its window.

Options:
  -w, --wait       Wait for the files to be closed before returning
  -h, --help       Show this help
  -v, --version    Show the version
  --               Treat everything after it as paths
";

/// One path to open, made absolute so another process can open it too.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Target {
    pub path: PathBuf,
    /// 1-based, like compiler messages.
    pub line: Option<usize>,
    pub col: Option<usize>,
}

impl Target {
    pub fn plain(path: PathBuf) -> Self {
        Self { path, line: None, col: None }
    }
}

#[derive(Debug, PartialEq)]
pub enum Cli {
    Open { targets: Vec<Target>, wait: bool },
    Help,
    Version,
    Error(String),
}

/// `args` excludes the program name; relative paths resolve against `cwd`.
pub fn parse(args: impl IntoIterator<Item = OsString>, cwd: &Path) -> Cli {
    let mut targets = Vec::new();
    let mut wait = false;
    let mut only_paths = false;
    for arg in args {
        let s = arg.to_string_lossy();
        if !only_paths && s.starts_with('-') && s.len() > 1 {
            match s.as_ref() {
                "--" => only_paths = true,
                "-w" | "--wait" => wait = true,
                "-h" | "--help" => return Cli::Help,
                "-v" | "--version" => return Cli::Version,
                "-n" | "--new-window" => return Cli::Error("Strata has a single window: -n/--new-window isn't supported".into()),
                // macOS adds this when an app is launched from Finder on old systems.
                _ if s.starts_with("-psn_") => {}
                _ => return Cli::Error(format!("unknown option {s}\n\n{USAGE}")),
            }
            continue;
        }
        targets.push(target(&arg, cwd));
    }
    Cli::Open { targets, wait }
}

/// `file`, `file:line` or `file:line:col`. A file whose name really ends in
/// `:12` wins over the line-number reading when it exists.
pub fn target(arg: &std::ffi::OsStr, cwd: &Path) -> Target {
    let whole = absolute(Path::new(arg), cwd);
    if whole.exists() {
        return Target::plain(whole);
    }
    let Some(s) = arg.to_str() else { return Target::plain(whole) };
    let num = |t: &str| (!t.is_empty() && t.bytes().all(|b| b.is_ascii_digit())).then(|| t.parse::<usize>().ok()).flatten();
    let mut parts = s.rsplitn(3, ':');
    let (last, mid, rest) = (parts.next(), parts.next(), parts.next());
    let (path, line, col) = match (last.and_then(num), mid.and_then(num), rest) {
        (Some(c), Some(l), Some(p)) if !p.is_empty() => (p.to_string(), Some(l), Some(c)),
        (Some(l), _, _) => match s.rsplit_once(':') {
            Some((p, _)) if !p.is_empty() => (p.to_string(), Some(l), None),
            _ => return Target::plain(whole),
        },
        _ => return Target::plain(whole),
    };
    Target { path: absolute(Path::new(&path), cwd), line: line.map(|l| l.max(1)), col: col.map(|c| c.max(1)) }
}

fn absolute(p: &Path, cwd: &Path) -> PathBuf {
    let joined = if p.is_absolute() { p.to_path_buf() } else { cwd.join(p) };
    // Drop `.` and `..` without touching the disk (the file may not exist yet).
    let mut out = PathBuf::new();
    for c in joined.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<OsString> {
        a.iter().map(OsString::from).collect()
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("strata-cli-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn line_and_column_suffixes() {
        let cwd = Path::new("/work");
        assert_eq!(target("a.rs:12".as_ref(), cwd), Target { path: "/work/a.rs".into(), line: Some(12), col: None });
        assert_eq!(target("src/a.rs:12:5".as_ref(), cwd), Target { path: "/work/src/a.rs".into(), line: Some(12), col: Some(5) });
        assert_eq!(target("/abs/./x/../a.rs:3".as_ref(), cwd), Target { path: "/abs/a.rs".into(), line: Some(3), col: None });
        // Not numbers: part of the name.
        assert_eq!(target("a:b".as_ref(), cwd), Target::plain("/work/a:b".into()));
        assert_eq!(target("a.rs:".as_ref(), cwd), Target::plain("/work/a.rs:".into()));
        assert_eq!(target(":12".as_ref(), cwd), Target::plain("/work/:12".into()));
        // Line 0 means the first line.
        assert_eq!(target("a:0:0".as_ref(), cwd).line, Some(1));
    }

    #[test]
    fn an_existing_file_with_a_colon_wins() {
        let d = tmp("colon");
        std::fs::write(d.join("notes:12"), "x").unwrap();
        assert_eq!(target("notes:12".as_ref(), &d), Target::plain(d.join("notes:12")));
        // A missing one is read as a line number.
        assert_eq!(target("other:12".as_ref(), &d), Target { path: d.join("other"), line: Some(12), col: None });
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn options_folders_and_several_paths() {
        let d = tmp("multi");
        std::fs::create_dir_all(d.join("proj")).unwrap();
        let Cli::Open { targets, wait } = parse(args(&["-w", "proj", "a.txt:3:4", "--", "-odd"]), &d) else { panic!() };
        assert!(wait);
        assert_eq!(targets, vec![Target::plain(d.join("proj")), Target { path: d.join("a.txt"), line: Some(3), col: Some(4) }, Target::plain(d.join("-odd"))]);
        assert!(targets[0].path.is_dir());
        assert_eq!(parse(args(&["--version"]), &d), Cli::Version);
        assert_eq!(parse(args(&["x", "-h"]), &d), Cli::Help);
        assert!(matches!(parse(args(&["-n"]), &d), Cli::Error(_)));
        assert!(matches!(parse(args(&["--bogus"]), &d), Cli::Error(_)));
        assert_eq!(parse(args(&["-psn_0_1234"]), &d), Cli::Open { targets: vec![], wait: false });
        std::fs::remove_dir_all(d).ok();
    }
}
