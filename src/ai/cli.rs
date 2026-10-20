//! The command-line providers: `claude -p` and `codex exec`, run with the
//! user's own login. Strata never reads or passes on those tools' login
//! files; it starts the program (found on PATH, plus the usual install
//! folders a GUI app doesn't see, as build systems are), writes the prompt
//! to its standard input and shows what it prints.
//!
//! Both run in an empty temporary folder. `claude` gets no tools at all
//! (`--tools ""`) and no MCP servers, so it can only answer from the text
//! it was given. `codex exec` has no way to turn its tools off, so it runs
//! with a read-only sandbox and is told to answer from the text alone.

use super::{Event, Kind, Prompt, Provider};
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::time::{Duration, Instant};

const MAX_OUTPUT: usize = 8 << 20;

/// Where `name` is: the first match on `path` (`PATH` when None), with
/// Windows' program extensions.
pub fn find_program(name: &str, path: Option<OsString>) -> Option<PathBuf> {
    let path = path.or_else(|| std::env::var_os("PATH"))?;
    let exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into()).split(';').filter(|e| !e.is_empty()).map(str::to_lowercase).collect()
    } else {
        vec![String::new()]
    };
    for dir in std::env::split_paths(&path) {
        if !dir.is_absolute() {
            continue; // a relative PATH entry would find programs in whatever folder is current
        }
        for ext in &exts {
            let p = dir.join(format!("{name}{ext}"));
            if is_program(&p) {
                return Some(p);
            }
        }
    }
    None
}

fn is_program(p: &Path) -> bool {
    let Ok(m) = std::fs::metadata(p) else { return false };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        m.is_file() && m.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    m.is_file()
}

/// The provider's program, looked up the way build systems are.
fn locate(provider: &Provider) -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(p) = &provider.program_for_tests {
        return Some(p.clone());
    }
    find_program(provider.kind.program(), crate::build::augmented_path())
}

/// The program's arguments. The prompt goes to standard input.
pub fn args(kind: Kind, model: &str, workdir: &Path) -> Vec<OsString> {
    let mut a: Vec<OsString> = Vec::new();
    match kind {
        Kind::CodexCli => {
            a.extend(["exec", "--sandbox", "read-only", "--skip-git-repo-check", "--color", "never", "-C"].map(OsString::from));
            a.push(workdir.as_os_str().to_owned());
            if !model.is_empty() {
                a.extend(["-m".into(), model.into()]);
            }
            a.push("-".into());
        }
        _ => {
            a.extend(["-p", "--tools", "", "--strict-mcp-config", "--no-session-persistence", "--output-format", "text"].map(OsString::from));
            if !model.is_empty() {
                a.extend(["--model".into(), model.into()]);
            }
        }
    }
    a
}

/// What the program reads: the instructions, then the material.
pub fn stdin_text(prompt: &Prompt) -> String {
    format!("{}\n\nAnswer from the text below alone: don't run commands, read files or look anything up.\n\n{}", prompt.system, prompt.user)
}

/// An empty folder of its own for one run.
fn work_dir() -> std::io::Result<PathBuf> {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!("strata-ai-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir(&dir)?;
    Ok(dir)
}

fn command(program: &Path, args: &[OsString], dir: &Path) -> Command {
    let mut cmd = Command::new(program);
    cmd.args(args).current_dir(dir).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(p) = crate::build::augmented_path() {
        cmd.env("PATH", p);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Its own process group, so cancel stops whatever it started too.
        cmd.process_group(0);
    }
    crate::update::fetch::no_window(&mut cmd);
    cmd
}

fn not_found(name: &str) -> String {
    format!("Couldn't find the {name} command. Install it and sign in, or check that it's on your PATH.")
}

/// Run the provider's program on `prompt`, streaming what it prints.
pub fn stream(provider: &Provider, prompt: &Prompt, cancel: &AtomicBool, emit: &mut dyn FnMut(Event)) {
    let name = provider.kind.program();
    let Some(program) = locate(provider) else { return emit(Event::Failed(not_found(name))) };
    let dir = match work_dir() {
        Ok(d) => d,
        Err(e) => return emit(Event::Failed(format!("Couldn't make a folder for {name}: {e}"))),
    };
    run(&program, name, &args(provider.kind, &provider.model, &dir), &dir, stdin_text(prompt), provider.stall, cancel, emit);
    let _ = std::fs::remove_dir_all(&dir);
}

#[allow(clippy::too_many_arguments)]
fn run(program: &Path, name: &str, args: &[OsString], dir: &Path, input: String, stall: Duration, cancel: &AtomicBool, emit: &mut dyn FnMut(Event)) {
    let mut child = match command(program, args, dir).spawn() {
        Ok(c) => c,
        Err(e) => return emit(Event::Failed(format!("Couldn't run {name}: {e}"))),
    };
    if let Some(mut stdin) = child.stdin.take() {
        std::thread::spawn(move || {
            let _ = stdin.write_all(input.as_bytes());
        });
    }
    let mut stderr = child.stderr.take();
    let errors = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(e) = stderr.as_mut() {
            let mut buf = Vec::new();
            let _ = e.take(16 * 1024).read_to_end(&mut buf);
            s = String::from_utf8_lossy(&buf).into_owned();
        }
        s
    });
    let (tx, rx) = channel::<Option<Vec<u8>>>();
    if let Some(mut stdout) = child.stdout.take() {
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match stdout.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send(Some(buf[..n].to_vec())).is_err() {
                            return;
                        }
                    }
                }
            }
            let _ = tx.send(None);
        });
    }
    let mut pending: Vec<u8> = Vec::new();
    let mut total = 0usize;
    let mut printed = false;
    let mut last = Instant::now();
    let stop = |child: &mut std::process::Child| {
        crate::build::kill_tree(child, true);
        let _ = child.wait();
    };
    loop {
        if cancel.load(Ordering::Relaxed) {
            stop(&mut child);
            return emit(Event::Failed(super::CANCELLED.into()));
        }
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(Some(bytes)) => {
                last = Instant::now();
                total += bytes.len();
                if total > MAX_OUTPUT {
                    stop(&mut child);
                    return emit(Event::Failed("The answer was far too long, so Strata stopped it.".into()));
                }
                pending.extend_from_slice(&bytes);
                let text = take_utf8(&mut pending);
                if !text.is_empty() {
                    printed |= !text.trim().is_empty();
                    emit(Event::Text(text));
                }
            }
            Ok(None) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {
                if last.elapsed() > stall {
                    stop(&mut child);
                    return emit(Event::Failed(format!("{name} printed nothing for {} seconds, so Strata stopped it (the ai_timeout_seconds setting).", stall.as_secs())));
                }
            }
        }
    }
    if !pending.is_empty() {
        emit(Event::Text(String::from_utf8_lossy(&pending).into_owned()));
    }
    let status = child.wait();
    let stderr = errors.join().unwrap_or_default();
    let last_line = stderr.lines().rev().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let detail: String = last_line.chars().filter(|c| !c.is_control()).take(300).collect();
    match status {
        Ok(s) if s.success() && printed => emit(Event::Done(None)),
        Ok(s) if s.success() => emit(Event::Failed(format!("{name} finished without an answer{}", if detail.is_empty() { ".".into() } else { format!(": {detail}") }))),
        Ok(s) => emit(Event::Failed(format!(
            "{name} failed ({}){}",
            s.code().map_or_else(|| "stopped".to_owned(), |c| format!("exit {c}")),
            if detail.is_empty() { ". Is it installed and signed in?".to_owned() } else { format!(": {detail}") }
        ))),
        Err(e) => emit(Event::Failed(format!("{name} failed: {e}"))),
    }
}

/// The text decoded so far, leaving an unfinished character in `pending`.
fn take_utf8(pending: &mut Vec<u8>) -> String {
    match std::str::from_utf8(pending) {
        Ok(s) => {
            let s = s.to_owned();
            pending.clear();
            s
        }
        Err(e) if e.error_len().is_none() => {
            let n = e.valid_up_to();
            let s = String::from_utf8_lossy(&pending[..n]).into_owned();
            pending.drain(..n);
            s
        }
        Err(_) => String::from_utf8_lossy(&std::mem::take(pending)).into_owned(),
    }
}

/// Test Connection for a command-line provider: is the program there, and
/// which version? Nothing is sent anywhere.
pub fn version(provider: &Provider) -> super::TestResult {
    let name = provider.kind.program();
    let program = locate(provider).ok_or_else(|| not_found(name))?;
    let dir = work_dir().map_err(|e| format!("Couldn't make a folder for {name}: {e}"))?;
    let mut out = String::new();
    let mut failed = None;
    let never = AtomicBool::new(false);
    run(&program, name, &["--version".into()], &dir, String::new(), Duration::from_secs(20), &never, &mut |e| match e {
        Event::Text(t) => out.push_str(&t),
        Event::Failed(f) => failed = Some(f),
        Event::Done(_) => {}
    });
    let _ = std::fs::remove_dir_all(&dir);
    if let Some(f) = failed {
        return Err(f);
    }
    let v: String = out.lines().next().unwrap_or("").trim().chars().filter(|c| !c.is_control()).take(120).collect();
    Ok(format!("Found {name} at {} ({v}). It sends to its vendor with your own login.", program.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locked_down_arguments() {
        let dir = Path::new("/tmp/w");
        let claude: Vec<String> = args(Kind::ClaudeCli, "", dir).into_iter().map(|a| a.into_string().unwrap()).collect();
        let tools = claude.iter().position(|a| a == "--tools").unwrap();
        assert_eq!(claude[tools + 1], "", "no tools: claude can't read files");
        assert!(claude.contains(&"--strict-mcp-config".to_owned()) && claude[0] == "-p");
        let codex: Vec<String> = args(Kind::CodexCli, "gpt-x", dir).into_iter().map(|a| a.into_string().unwrap()).collect();
        let sandbox = codex.iter().position(|a| a == "--sandbox").unwrap();
        assert_eq!(codex[sandbox + 1], "read-only");
        assert_eq!(codex.last().unwrap(), "-", "the prompt comes on stdin");
        assert!(codex.windows(2).any(|w| w == ["-m", "gpt-x"]));
    }

    #[test]
    fn utf8_split_across_reads() {
        let mut p = "héllo".as_bytes()[..2].to_vec();
        assert_eq!(take_utf8(&mut p), "h");
        p.extend_from_slice(&"héllo".as_bytes()[2..]);
        assert_eq!(take_utf8(&mut p), "éllo");
        assert!(p.is_empty());
    }

    #[test]
    fn relative_path_entries_are_skipped() {
        assert_eq!(find_program("definitely-not-a-program-strata", Some(OsString::from("."))), None);
    }
}
