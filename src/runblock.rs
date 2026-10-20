//! Running fenced code blocks from Markdown (issue #11): finding the block,
//! the languages that may run, capturing a block's output in a hidden
//! process, the `output` block that output goes into, and the files the
//! user said not to ask about again. The window side (the Run button, the
//! confirmation, the terminal) is in `app/runblock_ui.rs`.
//!
//! Nothing here runs on its own: a block runs only when the user clicks Run
//! or picks a command, and only after the confirmation the window shows.

use serde::{Deserialize, Serialize};
use std::io::Read;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How a language's blocks run in the terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Runs {
    /// Typed into a shell: the active terminal.
    Shell,
    /// Typed into the language's REPL, in a terminal tab of its own.
    Repl,
}

/// A language whose blocks get a Run button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lang {
    /// The name it's known by here (`js` is `node`, `powershell` stays itself).
    pub name: &'static str,
    pub runs: Runs,
}

/// Info-string words that run, and what they are. Anything else shows no button.
const LANGS: &[(&str, &str, Runs)] = &[
    ("bash", "bash", Runs::Shell),
    ("sh", "sh", Runs::Shell),
    ("zsh", "zsh", Runs::Shell),
    ("fish", "fish", Runs::Shell),
    ("pwsh", "pwsh", Runs::Shell),
    ("powershell", "powershell", Runs::Shell),
    ("cmd", "cmd", Runs::Shell),
    ("python", "python", Runs::Repl),
    ("node", "node", Runs::Repl),
    ("js", "node", Runs::Repl),
    ("ruby", "ruby", Runs::Repl),
];

/// The runnable language a block's info string names (its first word, any
/// case), if any.
pub fn lang(info_word: &str) -> Option<Lang> {
    LANGS.iter().find(|(w, ..)| w.eq_ignore_ascii_case(info_word)).map(|&(_, name, runs)| Lang { name, runs })
}

impl Lang {
    /// The REPL a language block starts in a new terminal tab.
    pub fn repl(self) -> Option<&'static str> {
        match self.name {
            "python" if cfg!(windows) => Some("python"),
            "python" => Some("python3"),
            "node" => Some("node"),
            // `ruby` alone reads a whole script before running it; irb is the REPL.
            "ruby" => Some("irb"),
            _ => None,
        }
    }
}

/// A fenced code block in a Markdown source.
#[derive(Clone, Debug, PartialEq)]
pub struct CodeBlock {
    /// Where the parser says the block starts: its opening fence. The
    /// preview names a block by this.
    pub at: usize,
    /// The block's whole lines in bytes, from the start of the opening
    /// fence's line to just after the closing fence's line break.
    pub lines: Range<usize>,
    /// The first word of the info string, as written (`bash` in ```` ```bash title="x" ````).
    pub lang: String,
    /// The code, with `\n` line breaks and no final one.
    pub code: String,
    /// What comes before the opening fence on its line when that's only
    /// spaces or tabs: an `output` block goes at the same indent. None when
    /// the fence follows a list marker or a quote's `>`.
    pub indent: Option<String>,
    /// It has a closing fence (an unclosed block runs to the end of its
    /// container, so it isn't offered).
    pub closed: bool,
}

/// Every fenced code block in `src`, in order, including those in lists and
/// quotes. Indented code blocks aren't included: they have no language.
/// `notes` parses as a note does (frontmatter isn't a block).
pub fn code_blocks(src: &str, notes: bool) -> Vec<CodeBlock> {
    use pulldown_cmark::{CodeBlockKind, Event, Tag, TagEnd};
    let mut out = Vec::new();
    let mut open: Option<(Range<usize>, String, String)> = None;
    for (ev, range) in pulldown_cmark::Parser::new_ext(src, crate::markdown::parser_options(notes)).into_offset_iter() {
        match ev {
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info))) => {
                let word = info.split(|c: char| c.is_whitespace() || c == ',' || c == '{').next().unwrap_or("").to_owned();
                open = Some((range, word, String::new()));
            }
            Event::Text(t) => {
                if let Some((_, _, text)) = &mut open {
                    text.push_str(&t);
                }
            }
            Event::End(TagEnd::CodeBlock) => {
                let Some((range, lang, text)) = open.take() else { continue };
                out.push(finish(src, range, lang, text));
            }
            _ => {}
        }
    }
    out
}

/// A fenced block whose source is `body` (from its opening fence, as the
/// parser gives it) and whose text is `text` (as the parser gives it) has a
/// closing fence. A closed block spans its opening line, its content lines
/// and its closing line; an unclosed one has no closing line. Counting lines
/// this way trusts the parser about what closes a fence (indents, lengths, `~`).
pub fn closed(body: &str, text: &str) -> bool {
    let spanned = body.matches('\n').count() + usize::from(!body.ends_with('\n'));
    let content = text.matches('\n').count() + usize::from(!text.is_empty() && !text.ends_with('\n'));
    spanned == content + 2
}

fn finish(src: &str, range: Range<usize>, lang: String, text: String) -> CodeBlock {
    let text = text.replace("\r\n", "\n");
    let closed = closed(&src[range.clone()], &text);
    let line_start = src[..range.start].rfind('\n').map_or(0, |i| i + 1);
    let prefix = &src[line_start..range.start];
    let indent = prefix.chars().all(|c| c == ' ' || c == '\t').then(|| prefix.to_owned());
    let rest = &src[range.end..];
    let end = range.end + if rest.starts_with("\r\n") { 2 } else { usize::from(rest.starts_with('\n')) };
    let mut code = text;
    if code.ends_with('\n') {
        code.pop();
    }
    CodeBlock { at: range.start, lines: line_start..end, lang, code, indent, closed }
}

/// The block holding byte `pos` of `src`: anywhere on its lines, fences
/// included, or at the very end of a file that ends inside it.
pub fn block_at(src: &str, blocks: &[CodeBlock], pos: usize) -> Option<usize> {
    // Blocks don't overlap (one in a list or quote is still one block).
    blocks.iter().position(|b| b.lines.contains(&pos) || (pos == b.lines.end && pos == src.len() && !src.ends_with('\n')))
}

/// The `output` block for block `i` holding `output`: the byte range of
/// `src` to replace and the text to put there. An `output` block right
/// under the code (on the next line, or after one blank line) at the same
/// indent is replaced; otherwise a new one goes on the next line. None when
/// the block can't take one (unclosed, or its fence shares a line with a list
/// marker or `>`). `eol` is the document's line break.
pub fn output_edit(src: &str, blocks: &[CodeBlock], i: usize, output: &str, eol: &str) -> Option<(Range<usize>, String)> {
    let b = blocks.get(i)?;
    let indent = b.indent.as_deref()?;
    if !b.closed {
        return None;
    }
    let after = b.lines.end;
    let fence = "`".repeat(fence_len(output));
    let mut block = format!("{indent}{fence}output{eol}");
    for line in output.lines() {
        if !line.is_empty() {
            block.push_str(indent);
            block.push_str(line);
        }
        block.push_str(eol);
    }
    block.push_str(&format!("{indent}{fence}"));
    let existing = blocks.get(i + 1).filter(|o| {
        let gap = &src[after.min(o.lines.start)..o.lines.start];
        o.lang.eq_ignore_ascii_case("output") && o.closed && o.indent.as_deref() == Some(indent) && o.lines.start >= after && (gap.is_empty() || (gap.ends_with('\n') && gap.trim().is_empty() && gap.matches('\n').count() == 1))
    });
    if let Some(o) = existing {
        // Keep whatever line break the old block ended with (none at the end of the file).
        let tail = &src[o.lines.clone()];
        let ending = if tail.ends_with("\r\n") { "\r\n" } else if tail.ends_with('\n') { "\n" } else { "" };
        return Some((o.lines.clone(), block + ending));
    }
    if src[..after].ends_with('\n') {
        Some((after..after, block + eol))
    } else {
        // The block ends the file without a line break: the output does too.
        Some((after..after, format!("{eol}{block}")))
    }
}

/// A backtick fence longer than any run of backticks starting a line of
/// `text`, so nothing in the output can close it early.
fn fence_len(text: &str) -> usize {
    let longest = text.lines().map(|l| l.trim_start().chars().take_while(|&c| c == '`').count()).max().unwrap_or(0);
    (longest + 1).max(3)
}

/// A character that hides or reorders what a block shows: bidirectional
/// controls (which can make `rm` read as something harmless), invisible
/// ones (zero-width, soft hyphen, byte order mark), and control characters
/// other than tab and line break (a lone carriage return, escape, delete,
/// the C1 range). A block holding any isn't run: the confirmation couldn't
/// show what would really run.
fn is_hidden(c: char) -> bool {
    matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}' | '\u{061C}')
        || matches!(c, '\u{200B}'..='\u{200D}' | '\u{2060}' | '\u{FEFF}' | '\u{00AD}')
        || (c.is_control() && c != '\t' && c != '\n')
}

/// The characters of `code` that keep it from running (see `is_hidden`),
/// each once, in order of appearance.
pub fn hidden(code: &str) -> Vec<char> {
    let mut found = Vec::new();
    for c in code.chars().filter(|&c| is_hidden(c)) {
        if !found.contains(&c) {
            found.push(c);
        }
    }
    found
}

/// `code` with each hidden character spelled out as ⟨U+XXXX⟩, for the confirmation.
pub fn visible(code: &str) -> String {
    let mut out = String::with_capacity(code.len());
    for c in code.chars() {
        if is_hidden(c) {
            out.push_str(&format!("⟨U+{:04X}⟩", c as u32));
        } else {
            out.push(c);
        }
    }
    out
}

// ---- capturing output ----------------------------------------------------------

/// The most output kept for the `output` block; the rest is counted, not kept.
pub const CAPTURE_LIMIT: usize = 256 * 1024;

/// What a captured run printed and how it ended.
#[derive(Debug)]
pub struct Captured {
    /// Standard output and error together, in the order they were written,
    /// cleaned of escape sequences, plus a note on how the run ended when
    /// it didn't end well.
    pub text: String,
    pub elapsed: Duration,
}

/// The interpreter a captured block runs under, the arguments before the
/// script's path, and the script's extension.
fn interpreter(lang: Lang) -> Result<(PathBuf, Vec<&'static str>, &'static str), String> {
    let (names, args, ext): (&[&str], Vec<&str>, &str) = match lang.name {
        "bash" => (&["bash"], vec![], "sh"),
        "sh" => (&["sh"], vec![], "sh"),
        "zsh" => (&["zsh"], vec![], "zsh"),
        "fish" => (&["fish"], vec![], "fish"),
        // Windows refuses unsigned scripts by default; this lifts that for this one run only.
        "pwsh" | "powershell" => {
            let names: &[&str] = if lang.name == "pwsh" { &["pwsh"] } else { &["powershell", "pwsh"] };
            let mut args = vec!["-NoProfile", "-NonInteractive"];
            if cfg!(windows) {
                args.extend(["-ExecutionPolicy", "Bypass"]);
            }
            args.push("-File");
            (names, args, "ps1")
        }
        "cmd" => (&["cmd"], vec!["/d", "/c"], "cmd"),
        "python" if cfg!(windows) => (&["python", "py"], vec![], "py"),
        "python" => (&["python3", "python"], vec![], "py"),
        "node" => (&["node"], vec![], "js"),
        "ruby" => (&["ruby"], vec![], "rb"),
        _ => return Err(format!("Strata can't run {} blocks", lang.name)),
    };
    let path = crate::build::augmented_path();
    names
        .iter()
        .find_map(|n| crate::ai::cli::find_program(n, path.clone()))
        .map(|p| (p, args, ext))
        .ok_or_else(|| format!("Couldn't find {} on this computer, so the block didn't run", names[0]))
}

/// The program a captured `lang` block runs under, or why there's none.
pub fn capture_program(lang: Lang) -> Result<PathBuf, String> {
    interpreter(lang).map(|(program, ..)| program)
}

/// A private folder of its own for one run's script.
fn script_dir() -> std::io::Result<PathBuf> {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!("strata-run-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    // Left over from a run that crashed. A link planted here is removed, not followed.
    let _ = std::fs::remove_dir_all(&dir);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        // Not recursive: fails if someone made it again in between.
        std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
    }
    #[cfg(not(unix))]
    std::fs::create_dir(&dir)?;
    Ok(dir)
}

/// Run `code` as a `lang` script in a new process in `dir` (not the user's
/// terminal), with no input, and collect what it prints: stdout and stderr
/// together, at most `limit` bytes kept. After `timeout` the process and
/// everything it started are stopped. Blocks until done; call it off the UI
/// thread.
pub fn capture(lang: Lang, code: &str, dir: &Path, timeout: Duration, limit: usize) -> Result<Captured, String> {
    let (program, args, ext) = interpreter(lang)?;
    let name = program.file_stem().map_or_else(|| lang.name.to_owned(), |s| s.to_string_lossy().into_owned());
    let scripts = script_dir().map_err(|e| format!("Couldn't make a folder for the script: {e}"))?;
    let result = run_script(&program, &name, &args, ext, code, &scripts, dir, timeout, limit);
    let _ = std::fs::remove_dir_all(&scripts);
    result
}

#[allow(clippy::too_many_arguments)]
fn run_script(program: &Path, name: &str, args: &[&str], ext: &str, code: &str, scripts: &Path, dir: &Path, timeout: Duration, limit: usize) -> Result<Captured, String> {
    let script = scripts.join(format!("block.{ext}"));
    // cmd.exe reads batch files line by line and wants CRLF.
    let body = if ext == "cmd" { code.replace('\n', "\r\n") } else { code.to_owned() } + if ext == "cmd" { "\r\n" } else { "\n" };
    crate::private::options()
        .write(true)
        .create_new(true)
        .open(&script)
        .and_then(|mut f| std::io::Write::write_all(&mut f, body.as_bytes()))
        .map_err(|e| format!("Couldn't write the script: {e}"))?;
    // One pipe for both, so they stay in the order they were written.
    let (reader, writer) = std::io::pipe().map_err(|e| format!("Couldn't make a pipe: {e}"))?;
    let writer2 = writer.try_clone().map_err(|e| format!("Couldn't make a pipe: {e}"))?;
    let mut cmd = Command::new(program);
    cmd.args(args).arg(&script).current_dir(dir).stdin(Stdio::null()).stdout(writer).stderr(writer2);
    if let Some(p) = crate::build::augmented_path() {
        cmd.env("PATH", p);
    }
    // Plain text: nothing is reading it as a terminal.
    cmd.env("NO_COLOR", "1").env("TERM", "dumb");
    // Python buffers its output when it isn't a terminal, which would put
    // everything printed to stderr ahead of it.
    cmd.env("PYTHONUNBUFFERED", "1");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Its own process group, so a timeout stops whatever it started too.
        cmd.process_group(0);
    }
    crate::update::fetch::no_window(&mut cmd);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Started suspended and resumed only once it's in the job (below), so
        // nothing it starts can get out first. Replaces no_window's flags.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const CREATE_SUSPENDED: u32 = 0x0000_0004;
        cmd.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
    }
    let started = Instant::now();
    let spawned = cmd.spawn();
    // Our copies of the write end must go, or the reader never sees the end.
    drop(cmd);
    let mut child = spawned.map_err(|e| format!("Couldn't run {name}: {e}"))?;
    let kept = Arc::new(Mutex::new((Vec::<u8>::new(), 0usize)));
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    {
        let kept = kept.clone();
        std::thread::spawn(move || {
            let mut reader = reader;
            let mut buf = [0u8; 16 * 1024];
            loop {
                let n = match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                let mut k = kept.lock().unwrap_or_else(|e| e.into_inner());
                // Keep reading past the limit (counting only), so a chatty
                // command isn't blocked on a full pipe and finishes as it would.
                let room = limit.saturating_sub(k.0.len());
                k.0.extend_from_slice(&buf[..n.min(room)]);
                k.1 += n;
            }
            let _ = done_tx.send(());
        });
    }
    #[cfg(windows)]
    let job = match crate::lsp::server::job::Job::holding(&child) {
        Some(job) if tree::resume(&child) => job,
        // Not contained, so not run: it's still suspended and hasn't done anything.
        other => {
            drop(other);
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("Couldn't run {name}: Windows wouldn't let Strata keep track of the processes it starts, so the block didn't run"));
        }
    };
    let mut timed_out = false;
    // Wait for the interpreter to end without reaping it: until it's reaped
    // its process id, which is also its process group's, can't be reused, so
    // signalling the group below can't reach anyone else.
    loop {
        if tree::ended(&child) {
            break;
        }
        if started.elapsed() >= timeout {
            timed_out = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    // Whether it ended or timed out, nothing it started outlives the block:
    // its process group (background jobs, pipelines), and on a timeout also
    // the descendants that moved to groups of their own while it's alive to
    // find them by.
    tree::stop(&child, timed_out);
    #[cfg(windows)]
    job.terminate();
    if timed_out {
        crate::build::kill_tree(&mut child, true);
    }
    let status = child.wait().ok();
    let elapsed = started.elapsed();
    // Let the reader drain what's buffered. Something the block left running
    // in the background may hold the pipe open; its output isn't waited for.
    let _ = done_rx.recv_timeout(Duration::from_secs(1));
    let (bytes, total) = std::mem::take(&mut *kept.lock().unwrap_or_else(|e| e.into_inner()));
    let mut text = clean_output(&bytes, total > bytes.len());
    let mut notes = Vec::new();
    if total > bytes.len() {
        notes.push(format!("[Strata kept the first {} of {} of output]", size(bytes.len()), size(total)));
    }
    if timed_out {
        notes.push(format!("[Strata stopped it after {}]", seconds(timeout)));
    } else {
        match status.map(|s| s.code()) {
            Some(Some(0)) => {}
            Some(Some(code)) => notes.push(format!("[exit status {code}]")),
            Some(None) => notes.push("[stopped by a signal]".into()),
            None => notes.push("[Strata lost track of the process]".into()),
        }
    }
    for n in notes {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&n);
    }
    Ok(Captured { text, elapsed })
}

/// Ending what a captured block started.
mod tree {
    use std::process::Child;

    /// The interpreter has ended (it's left unreaped, see `run_script`).
    #[cfg(unix)]
    pub fn ended(child: &Child) -> bool {
        // SAFETY: waitid fills `info`, which outlives the call; WNOWAIT leaves the child to `wait`.
        unsafe {
            let mut info: libc::siginfo_t = std::mem::zeroed();
            let r = libc::waitid(libc::P_PID, child.id() as libc::id_t, &mut info, libc::WEXITED | libc::WNOHANG | libc::WNOWAIT);
            // An error (no such child) counts as ended too: there's nothing left to wait for.
            r != 0 || info.si_pid() != 0
        }
    }

    #[cfg(not(unix))]
    pub fn ended(child: &Child) -> bool {
        // SAFETY: a handle we own; a zero-timeout wait only asks.
        #[cfg(windows)]
        unsafe {
            use std::os::windows::io::AsRawHandle;
            windows_sys::Win32::System::Threading::WaitForSingleObject(child.as_raw_handle() as _, 0) == 0
        }
        #[cfg(not(windows))]
        {
            let _ = child;
            true
        }
    }

    /// Kill the run's process group and, with `descendants`, every process
    /// the interpreter started that's still its descendant, wherever its
    /// group. Everything is frozen first: the kernel signals a group one
    /// process at a time, so a shell whose `sleep` was killed first could
    /// otherwise go on to its next command before its own signal lands.
    #[cfg(unix)]
    pub fn stop(child: &Child, descendants: bool) {
        let Ok(pgid) = i32::try_from(child.id()) else { return };
        // SAFETY: kill(2) on our unreaped child's own process group; no memory involved.
        unsafe {
            libc::kill(-pgid, libc::SIGSTOP);
        }
        let mut found: Vec<Seen> = Vec::new();
        if descendants {
            // Each is recorded with its parent and start time as it's found,
            // and checked again right before each signal, so a process id
            // that was reused for someone else in between is left alone.
            // Freezing each as it's found keeps it from starting others.
            let mut queue = vec![pgid];
            while let Some(p) = queue.pop() {
                for c in children(p) {
                    if c <= 1 || found.iter().any(|f| f.pid == c) {
                        continue;
                    }
                    let Some(s) = seen(c).filter(|s| s.ppid == p) else { continue };
                    // SAFETY: a process just verified to be this child of the tree.
                    if still(&s) && unsafe { libc::kill(c, libc::SIGSTOP) } == 0 {
                        found.push(s);
                        queue.push(c);
                    }
                }
            }
        }
        // The descendants go before the group: once their parents die they
        // would be handed to init and no longer match what was recorded.
        for s in &found {
            if still(s) {
                // SAFETY: verified just now; it's frozen, so it can't exit and
                // free its id between the check and the signal.
                if unsafe { libc::kill(s.pid, libc::SIGKILL) } != 0 {
                    log::warn!("couldn't stop process {} started by a code block: {}", s.pid, std::io::Error::last_os_error());
                }
            }
        }
        // SAFETY: as above.
        if unsafe { libc::kill(-pgid, libc::SIGKILL) } != 0 {
            let e = std::io::Error::last_os_error();
            // No such group: everything in it has already ended.
            if e.raw_os_error() != Some(libc::ESRCH) {
                log::warn!("couldn't stop a code block's processes: {e}");
            }
        }
    }

    /// A process as it was when found: its id, its parent's, and when it
    /// started (which a reused id doesn't share).
    #[cfg(unix)]
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub(super) struct Seen {
        pub pid: i32,
        pub ppid: i32,
        pub start: u64,
    }

    /// `s`'s process is still the one that was found: same parent, same start.
    #[cfg(unix)]
    pub(super) fn still(s: &Seen) -> bool {
        seen(s.pid).as_ref() == Some(s)
    }

    #[cfg(target_os = "macos")]
    pub(super) fn seen(pid: i32) -> Option<Seen> {
        // SAFETY: proc_pidinfo fills `info` up to the size given; zeroed is a valid value.
        unsafe {
            let mut info: libc::proc_bsdinfo = std::mem::zeroed();
            let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
            let n = libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&mut info as *mut libc::proc_bsdinfo).cast(), size);
            (n == size && info.pbi_pid as i32 == pid).then(|| Seen { pid, ppid: info.pbi_ppid as i32, start: info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec })
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    pub(super) fn seen(pid: i32) -> Option<Seen> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // After the name in parentheses: state, parent (field 4), …, start time (field 22).
        let rest: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
        Some(Seen { pid, ppid: rest.get(1)?.parse().ok()?, start: rest.get(19)?.parse().ok()? })
    }

    #[cfg(not(unix))]
    pub fn stop(_child: &Child, _descendants: bool) {}

    /// Resume the threads of `child`, started suspended. False if there
    /// were none to resume or one couldn't be.
    #[cfg(windows)]
    pub fn resume(child: &Child) -> bool {
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next};
        use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};
        let pid = child.id();
        let mut resumed = 0;
        // SAFETY: plain Win32 calls on handles we open and close here; `entry` outlives them.
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snap == INVALID_HANDLE_VALUE {
                return false;
            }
            let mut entry: THREADENTRY32 = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
            let mut more = Thread32First(snap, &mut entry) != 0;
            while more {
                if entry.th32OwnerProcessID == pid {
                    let t = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                    if t.is_null() || ResumeThread(t) == u32::MAX {
                        if !t.is_null() {
                            CloseHandle(t);
                        }
                        CloseHandle(snap);
                        return false;
                    }
                    CloseHandle(t);
                    resumed += 1;
                }
                more = Thread32Next(snap, &mut entry) != 0;
            }
            CloseHandle(snap);
        }
        resumed > 0
    }

    /// The processes whose parent is `pid`.
    #[cfg(target_os = "macos")]
    fn children(pid: i32) -> Vec<i32> {
        unsafe extern "C" {
            fn proc_listpids(kind: u32, typeinfo: u32, buffer: *mut std::ffi::c_void, size: i32) -> i32;
        }
        const PROC_PPID_ONLY: u32 = 6;
        let mut buf = vec![0i32; 1024];
        // SAFETY: the buffer is as long as the size given; the result is bytes filled.
        let n = unsafe { proc_listpids(PROC_PPID_ONLY, pid as u32, buf.as_mut_ptr().cast(), (buf.len() * 4) as i32) };
        buf.truncate((n.max(0) as usize / 4).min(1024));
        buf.retain(|&p| p > 0);
        buf
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    fn children(pid: i32) -> Vec<i32> {
        let Ok(dir) = std::fs::read_dir("/proc") else { return Vec::new() };
        dir.flatten()
            .filter_map(|e| e.file_name().to_str()?.parse::<i32>().ok())
            .filter(|p| {
                seen(*p).is_some_and(|s| s.ppid == pid)
            })
            .collect()
    }
}

fn size(n: usize) -> String {
    match n {
        n if n < 1024 => format!("{n} bytes"),
        n if n < 1 << 20 => format!("{} KB", n / 1024),
        n => format!("{:.1} MB", n as f64 / (1u64 << 20) as f64),
    }
}

fn seconds(d: Duration) -> String {
    let s = d.as_secs_f64();
    if s.fract() == 0.0 { format!("{} s", s as u64) } else { format!("{s:.1} s") }
}

/// Output as text for a Markdown block: invalid UTF-8 replaced, escape
/// sequences (colors, titles) dropped, a carriage return overwriting the
/// line as a terminal would (progress bars), other control characters
/// dropped, and no trailing blank lines. `cut`: the bytes stop mid-output,
/// so a partial last line is dropped.
pub fn clean_output(bytes: &[u8], cut: bool) -> String {
    let mut bytes = bytes;
    if cut && let Some(nl) = bytes.iter().rposition(|&b| b == b'\n') {
        bytes = &bytes[..nl + 1];
    }
    let raw = String::from_utf8_lossy(bytes);
    let mut plain = String::with_capacity(raw.len());
    let mut it = raw.chars().peekable();
    while let Some(c) = it.next() {
        if c != '\u{1b}' {
            plain.push(c);
            continue;
        }
        match it.next() {
            // CSI: parameters, then a final byte from @ to ~.
            Some('[') => {
                for c in it.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            // OSC, DCS, APC, PM: up to BEL or ESC \.
            Some(']' | 'P' | '_' | '^') => {
                while let Some(c) = it.next() {
                    if c == '\u{7}' {
                        break;
                    }
                    if c == '\u{1b}' && it.peek() == Some(&'\\') {
                        it.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    let mut out = String::with_capacity(plain.len());
    for line in plain.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let line = line.rsplit('\r').next().unwrap_or(line);
        out.extend(line.chars().filter(|&c| c == '\t' || !c.is_control()));
        out.push('\n');
    }
    out.truncate(out.trim_end().len());
    out
}

// ---- files not to ask about again ------------------------------------------------

/// Which file a run came from, as it was: its real path, its identity on
/// disk (device, file number and creation time) and the SHA-256 of its
/// bytes. A different file put at the same path, or the same file changed by
/// another program (in place, or through a hard link to it), isn't taken for
/// the one that was allowed; Strata's own saves carry the permission over
/// (see `Allowed::moved`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileKey {
    pub path: PathBuf,
    pub id: String,
    /// A list saved before this field existed has none: those entries ask again.
    #[serde(default)]
    pub sha256: String,
}

/// The key of the file at `path`, or None when it isn't a file on disk.
pub fn file_key(path: &Path) -> Option<FileKey> {
    let real = std::fs::canonicalize(path).ok()?;
    let meta = std::fs::metadata(&real).ok()?;
    if !meta.is_file() {
        return None;
    }
    let (volume, node) = crate::claim::file_id(&real)?;
    let born = meta.created().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
    let sha256 = sha256_of(&real)?;
    Some(FileKey { path: real, id: format!("{volume}:{node}:{born}"), sha256 })
}

/// The SHA-256 of a file's bytes, in hex, read a piece at a time (opened
/// without following a link at the end).
fn sha256_of(path: &Path) -> Option<String> {
    use sha2::Digest;
    let mut f = crate::claim::open_plain(path).ok()?;
    let mut h = sha2::Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        match f.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => h.update(&buf[..n]),
            Err(_) => return None,
        }
    }
    Some(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// The list of files not to ask about again, in Strata's config folder.
pub const ALLOWED_FILE: &str = "code-blocks-allowed.json";

#[derive(Serialize, Deserialize, Default)]
struct Saved {
    #[serde(default)]
    files: Vec<FileKey>,
}

/// Files whose code blocks run without asking: "don't ask again" ones, kept
/// in `code-blocks-allowed.json` in Strata's config folder (never in the
/// file), and ones confirmed once this session. The saved list is read
/// fresh at every check, so another window's answer counts and a deleted
/// list means "ask again".
#[derive(Default)]
pub struct Allowed {
    /// None: nothing is saved (tests, no config folder); `memory` holds it.
    path: Option<PathBuf>,
    memory: Vec<FileKey>,
    session: Vec<FileKey>,
}

impl Allowed {
    pub fn new(config: Option<&Path>) -> Self {
        Allowed { path: config.map(|d| d.join(ALLOWED_FILE)), ..Default::default() }
    }

    fn load(&self) -> Vec<FileKey> {
        match &self.path {
            Some(p) => std::fs::read_to_string(p).ok().and_then(|t| serde_json::from_str::<Saved>(&t).ok()).map(|s| s.files).unwrap_or_default(),
            None => self.memory.clone(),
        }
    }

    pub fn allowed(&self, key: &FileKey) -> bool {
        self.session.contains(key) || self.load().contains(key)
    }

    /// Don't ask again this session.
    pub fn allow_for_session(&mut self, key: FileKey) {
        if !self.session.contains(&key) {
            self.session.push(key);
        }
    }

    /// Don't ask again at all ("don't ask again for this file").
    pub fn allow_always(&mut self, key: FileKey) -> Result<(), String> {
        let mut all = self.load();
        if !all.contains(&key) {
            all.push(key.clone());
        }
        self.allow_for_session(key);
        self.save(all)
    }

    /// Saving replaces the file (a new identity on disk): what was allowed
    /// for the old one carries over to what Strata wrote in its place.
    pub fn moved(&mut self, old: &FileKey, new: &FileKey) -> Result<(), String> {
        for k in &mut self.session {
            if k == old {
                *k = new.clone();
            }
        }
        let mut all = self.load();
        if !all.contains(old) {
            return Ok(());
        }
        all.retain(|k| k != old && k != new);
        all.push(new.clone());
        self.save(all)
    }

    /// Forget every file: each asks again. Returns how many were remembered.
    pub fn forget_all(&mut self) -> Result<usize, String> {
        let n = self.load().len();
        self.session.clear();
        self.save(Vec::new())?;
        Ok(n)
    }

    fn save(&mut self, files: Vec<FileKey>) -> Result<(), String> {
        let Some(path) = &self.path else {
            self.memory = files;
            return Ok(());
        };
        let text = serde_json::to_string_pretty(&Saved { files }).map_err(|e| e.to_string())? + "\n";
        // A sibling renamed over it, so a crash can't leave half a file;
        // owner-only, as it holds paths.
        let tmp = path.with_extension("json.tmp");
        let written = path.parent().map_or(Ok(()), crate::private::create_dir_all).and_then(|()| crate::private::write(&tmp, text)).and_then(|()| std::fs::rename(&tmp, path));
        written.map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("Couldn't save {}: {e}", path.display())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("strata-runblock-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn one(src: &str) -> CodeBlock {
        let b = code_blocks(src, false);
        assert_eq!(b.len(), 1, "{src:?}: {b:?}");
        b.into_iter().next().unwrap()
    }

    #[test]
    fn finds_fenced_blocks_with_their_lines() {
        let src = "# Run\n\n```bash\nls -l\necho hi\n```\n\nafter\n";
        let b = one(src);
        assert_eq!((b.lang.as_str(), b.code.as_str(), b.closed), ("bash", "ls -l\necho hi", true));
        assert_eq!(&src[b.lines.clone()], "```bash\nls -l\necho hi\n```\n");
        assert_eq!(b.indent.as_deref(), Some(""));
        assert_eq!(b.at, 7);
    }

    #[test]
    fn nested_fences_are_one_block() {
        // A longer fence holds a shorter one as text.
        let b = one("````markdown\n```bash\nrm -rf /\n```\n````\n");
        assert_eq!(b.lang, "markdown");
        assert_eq!(b.code, "```bash\nrm -rf /\n```");
        assert!(b.closed);
        // A shorter fence doesn't close a longer one.
        let b = one("````sh\necho a\n```\necho b\n````\n");
        assert_eq!(b.code, "echo a\n```\necho b");
    }

    #[test]
    fn tilde_fences_and_info_strings() {
        let b = one("~~~bash\necho tilde\n~~~\n");
        assert_eq!((b.lang.as_str(), b.code.as_str(), b.closed), ("bash", "echo tilde", true));
        let b = one("```bash title=\"x\"\necho t\n```\n");
        assert_eq!(b.lang, "bash");
        let b = one("```python {.numberLines}\nprint(1)\n```\n");
        assert_eq!(b.lang, "python");
        let b = one("```sh,ignore\nexit 1\n```\n");
        assert_eq!(b.lang, "sh");
        // A backtick fence's info string can't hold a backtick, so this is text.
        assert!(code_blocks("```ba`sh\nx\n```\n", false).iter().all(|b| b.lang != "ba`sh"));
    }

    #[test]
    fn indented_blocks_are_left_out() {
        assert!(code_blocks("Text\n\n    echo indented\n    ls\n", false).is_empty());
        // Indented a little, a fence is still a fence (and the output goes at that indent).
        let b = one("  ```sh\n  echo a\n   echo b\n  ```\n");
        assert_eq!((b.code.as_str(), b.indent.as_deref()), ("echo a\n echo b", Some("  ")));
    }

    #[test]
    fn crlf_files() {
        let src = "Intro\r\n\r\n```bash\r\nls\r\necho hi\r\n```\r\nafter\r\n";
        let b = one(src);
        assert_eq!(b.code, "ls\necho hi");
        assert!(b.closed);
        assert_eq!(&src[b.lines.clone()], "```bash\r\nls\r\necho hi\r\n```\r\n");
    }

    #[test]
    fn unclosed_lists_and_quotes() {
        assert!(!one("```bash\necho never closed\n").closed);
        assert!(!one("```bash\necho never closed").closed);
        assert!(!one("```bash\nls\n\n").closed);
        // Indented four spaces, a fence is text, so this one never closes.
        assert!(!one("```bash\nls\n    ```").closed);
        assert!(one("```bash\n```").closed, "an empty block at the end of the file");
        // In a list item: runs, and takes output at the item's indent.
        let b = one("1. Install:\n\n   ```sh\n   make install\n   ```\n");
        assert_eq!((b.code.as_str(), b.indent.as_deref(), b.closed), ("make install", Some("   "), true));
        // After a list marker or in a quote: runs, but has nowhere plain to put output.
        let b = one("- ```sh\n  ls\n  ```\n");
        assert_eq!((b.code.as_str(), b.indent.as_deref(), b.closed), ("ls", None, true));
        let b = one("> ```sh\n> ls\n> ```\n");
        assert_eq!((b.code.as_str(), b.indent.as_deref(), b.closed), ("ls", None, true));
    }

    #[test]
    fn notes_frontmatter_isnt_a_block() {
        let src = "---\ntags: [x]\n---\n\n```sh\necho note\n```\n";
        let b = code_blocks(src, true);
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].code, "echo note");
    }

    #[test]
    fn the_block_at_the_cursor() {
        let src = "a\n```sh\necho 1\n```\nb\n```sh\necho 2\n```";
        let blocks = code_blocks(src, false);
        let at = |pos: usize| block_at(src, &blocks, pos);
        assert_eq!(at(0), None);
        assert_eq!(at(2), Some(0), "on the opening fence");
        assert_eq!(at(src.find("echo 1").unwrap()), Some(0));
        assert_eq!(at(src.find("```\nb").unwrap() + 3), Some(0), "at the end of the closing fence");
        assert_eq!(at(src.find("b\n").unwrap()), None);
        assert_eq!(at(src.len()), Some(1), "the end of a file that ends in the block");
    }

    #[test]
    fn languages_that_run() {
        for w in ["bash", "sh", "zsh", "fish", "pwsh", "powershell", "cmd"] {
            assert_eq!(lang(w).map(|l| l.runs), Some(Runs::Shell), "{w}");
        }
        for w in ["python", "node", "js", "ruby"] {
            assert_eq!(lang(w).map(|l| l.runs), Some(Runs::Repl), "{w}");
        }
        assert_eq!(lang("JS").map(|l| l.name), Some("node"));
        assert_eq!(lang("Bash").map(|l| l.name), Some("bash"));
        for w in ["", "rust", "console", "shell", "text", "output", "python3", "javascript", "bat", "sql", "html", "bash2"] {
            assert_eq!(lang(w), None, "{w}");
        }
        assert_eq!(lang("ruby").and_then(Lang::repl), Some("irb"));
        assert_eq!(lang("js").and_then(Lang::repl), Some("node"));
        assert_eq!(lang("bash").and_then(Lang::repl), None);
    }

    /// Apply an edit to a string.
    fn apply(src: &str, edit: &(Range<usize>, String)) -> String {
        format!("{}{}{}", &src[..edit.0.start], edit.1, &src[edit.0.end..])
    }

    #[test]
    fn output_goes_under_the_block_and_replaces_the_old_one() {
        let src = "# Steps\n\n```sh\necho hi\n```\n\nNext step.\n";
        let blocks = code_blocks(src, false);
        let e = output_edit(src, &blocks, 0, "hi\n\nthere", "\n").unwrap();
        let once = apply(src, &e);
        assert_eq!(once, "# Steps\n\n```sh\necho hi\n```\n```output\nhi\n\nthere\n```\n\nNext step.\n");
        // Again: the output block right under it is replaced, not added to.
        let blocks = code_blocks(&once, false);
        let e = output_edit(&once, &blocks, 0, "bye", "\n").unwrap();
        assert_eq!(apply(&once, &e), "# Steps\n\n```sh\necho hi\n```\n```output\nbye\n```\n\nNext step.\n");
        // One blank line between counts as right under; a paragraph between doesn't.
        let spaced = "```sh\necho hi\n```\n\n```output\nold\n```\n";
        let e = output_edit(spaced, &code_blocks(spaced, false), 0, "new", "\n").unwrap();
        assert_eq!(apply(spaced, &e), "```sh\necho hi\n```\n\n```output\nnew\n```\n");
        let apart = "```sh\necho hi\n```\n\nSee:\n\n```output\nkeep me\n```\n";
        let e = output_edit(apart, &code_blocks(apart, false), 0, "new", "\n").unwrap();
        assert!(apply(apart, &e).contains("keep me") && apply(apart, &e).starts_with("```sh\necho hi\n```\n```output\nnew\n```\n"));
        // An unclosed output block isn't replaced (that would swallow the rest of the file).
        let open = "```sh\necho hi\n```\n```output\nrest of the file\n";
        let e = output_edit(open, &code_blocks(open, false), 0, "new", "\n").unwrap();
        assert!(apply(open, &e).ends_with("```output\nrest of the file\n"));
        // Only an output block counts.
        let other = "```sh\necho hi\n```\n```sh\necho two\n```\n";
        let e = output_edit(other, &code_blocks(other, false), 0, "new", "\n").unwrap();
        assert!(apply(other, &e).contains("echo two"));
    }

    #[test]
    fn output_fence_indent_crlf_and_end_of_file() {
        // Output with a fence in it gets a longer fence.
        let src = "```sh\ncat x.md\n```\n";
        let e = output_edit(src, &code_blocks(src, false), 0, "```rust\nfn x() {}\n```", "\n").unwrap();
        let out = apply(src, &e);
        assert_eq!(out, "```sh\ncat x.md\n```\n````output\n```rust\nfn x() {}\n```\n````\n");
        assert_eq!(code_blocks(&out, false).len(), 2, "still two blocks");
        // In a list item, at the item's indent.
        let src = "1. Go:\n\n   ```sh\n   pwd\n   ```\n2. Next\n";
        let e = output_edit(src, &code_blocks(src, false), 0, "/tmp\n\nend", "\n").unwrap();
        assert_eq!(apply(src, &e), "1. Go:\n\n   ```sh\n   pwd\n   ```\n   ```output\n   /tmp\n\n   end\n   ```\n2. Next\n");
        // CRLF documents get CRLF.
        let src = "```sh\r\necho a\r\n```\r\nnext\r\n";
        let e = output_edit(src, &code_blocks(src, false), 0, "a\nb", "\r\n").unwrap();
        assert_eq!(apply(src, &e), "```sh\r\necho a\r\n```\r\n```output\r\na\r\nb\r\n```\r\nnext\r\n");
        let twice = apply(src, &e);
        let e = output_edit(&twice, &code_blocks(&twice, false), 0, "c", "\r\n").unwrap();
        assert_eq!(apply(&twice, &e), "```sh\r\necho a\r\n```\r\n```output\r\nc\r\n```\r\nnext\r\n");
        // At the end of a file without a final line break, none is added.
        let src = "```sh\necho a\n```";
        let e = output_edit(src, &code_blocks(src, false), 0, "a", "\n").unwrap();
        let out = apply(src, &e);
        assert_eq!(out, "```sh\necho a\n```\n```output\na\n```");
        let e = output_edit(&out, &code_blocks(&out, false), 0, "b", "\n").unwrap();
        assert_eq!(apply(&out, &e), "```sh\necho a\n```\n```output\nb\n```");
        // Empty output is an empty block.
        let src = "```sh\ntrue\n```\n";
        let e = output_edit(src, &code_blocks(src, false), 0, "", "\n").unwrap();
        assert_eq!(apply(src, &e), "```sh\ntrue\n```\n```output\n```\n");
        // No place for it: unclosed, or after a list marker.
        let src = "```sh\nls\n";
        assert!(output_edit(src, &code_blocks(src, false), 0, "x", "\n").is_none());
        let src = "- ```sh\n  ls\n  ```\n";
        assert!(output_edit(src, &code_blocks(src, false), 0, "x", "\n").is_none());
    }

    #[test]
    fn output_is_cleaned() {
        assert_eq!(clean_output(b"\x1b[31mred\x1b[0m plain\n", false), "red plain");
        assert_eq!(clean_output(b"\x1b]0;title\x07after\n", false), "after");
        assert_eq!(clean_output(b"10%\r50%\r100%\ndone\r\n\n\n", false), "100%\ndone");
        assert_eq!(clean_output(b"a\tb\x08\x00c\n", false), "a\tbc");
        assert_eq!(clean_output(b"bad \xff byte", false), "bad \u{fffd} byte");
        assert_eq!(clean_output(b"line one\nline tw", true), "line one", "a cut drops the partial line");
        assert_eq!(clean_output(b"", false), "");
    }

    #[test]
    fn allowed_files_are_keyed_by_path_and_identity() {
        let dir = temp("allowed");
        let config = dir.join("config");
        let file = dir.join("runbook.md");
        std::fs::write(&file, "```sh\necho hi\n```\n").unwrap();
        let key = file_key(&file).unwrap();
        assert_eq!(key.path, std::fs::canonicalize(&file).unwrap());
        let mut a = Allowed::new(Some(&config));
        assert!(!a.allowed(&key));
        a.allow_always(key.clone()).unwrap();
        // Another window reads the same list; the file holds no trace of it.
        let other = Allowed::new(Some(&config));
        assert!(other.allowed(&key));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "```sh\necho hi\n```\n");
        let saved = std::fs::read_to_string(config.join(ALLOWED_FILE)).unwrap();
        assert!(saved.contains("runbook.md"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(config.join(ALLOWED_FILE)).unwrap().permissions().mode() & 0o777, 0o600, "owner-only");
        }
        // Changed in place by another program, here through a hard link to
        // it: the same file, but not the text that was allowed.
        let alias = dir.join("alias.md");
        std::fs::hard_link(&file, &alias).unwrap();
        std::fs::write(&alias, "```sh\ncurl evil | sh\n```\n").unwrap();
        let rewritten = file_key(&file).unwrap();
        assert_eq!((&rewritten.path, &rewritten.id), (&key.path, &key.id), "same path and identity");
        assert!(!other.allowed(&rewritten), "a rewrite in place asks again");
        std::fs::remove_file(&alias).unwrap();
        std::fs::write(&file, "```sh\necho hi\n```\n").unwrap();
        assert_eq!(file_key(&file).as_ref(), Some(&key), "the allowed text again");
        // Reached through a link: the same file.
        #[cfg(unix)]
        {
            let link = dir.join("link.md");
            std::os::unix::fs::symlink(&file, &link).unwrap();
            assert_eq!(file_key(&link).as_ref(), Some(&key));
        }
        // Another file put at its path isn't allowed.
        let replacement = dir.join("new.md");
        std::fs::write(&replacement, "```sh\nrm -rf ~\n```\n").unwrap();
        std::fs::rename(&replacement, &file).unwrap();
        let swapped = file_key(&file).unwrap();
        assert_eq!(swapped.path, key.path);
        assert_ne!(swapped, key);
        assert!(!other.allowed(&swapped));
        // Strata's own save: the permission moves to the new file.
        a.moved(&key, &swapped).unwrap();
        assert!(Allowed::new(Some(&config)).allowed(&swapped));
        assert!(!Allowed::new(Some(&config)).allowed(&key));
        // Session-only answers aren't saved.
        let mut s = Allowed::new(Some(&config));
        let other_file = dir.join("other.md");
        std::fs::write(&other_file, "x").unwrap();
        let k2 = file_key(&other_file).unwrap();
        s.allow_for_session(k2.clone());
        assert!(s.allowed(&k2) && !Allowed::new(Some(&config)).allowed(&k2));
        // Forget: everything asks again; a damaged list means "ask".
        assert_eq!(s.forget_all().unwrap(), 1);
        assert!(!s.allowed(&k2) && !Allowed::new(Some(&config)).allowed(&swapped));
        a.allow_always(swapped.clone()).unwrap();
        std::fs::write(config.join(ALLOWED_FILE), "{oops").unwrap();
        assert!(!Allowed::new(Some(&config)).allowed(&swapped));
        assert!(file_key(&dir).is_none(), "a folder has no key");
        assert!(file_key(&dir.join("missing.md")).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn sh() -> Lang {
        lang("sh").unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn capture_runs_a_real_shell() {
        let dir = temp("capture");
        let out = capture(sh(), "echo out\necho err >&2\necho \"in $(basename \"$PWD\")\"\nprintf 'no newline'", &dir, Duration::from_secs(20), CAPTURE_LIMIT).unwrap();
        assert_eq!(out.text, format!("out\nerr\nin {}\nno newline", dir.file_name().unwrap().to_string_lossy()));
        // No input: a read gets the end of input at once.
        let out = capture(sh(), "read x || echo no-input", &dir, Duration::from_secs(20), CAPTURE_LIMIT).unwrap();
        assert_eq!(out.text, "no-input");
        let out = capture(sh(), "echo failing; exit 3", &dir, Duration::from_secs(20), CAPTURE_LIMIT).unwrap();
        assert_eq!(out.text, "failing\n[exit status 3]");
        // Colors are dropped (and NO_COLOR is set for programs that look).
        let out = capture(sh(), "printf '\\033[1;32mok\\033[0m\\n'; echo \"$NO_COLOR $TERM\"", &dir, Duration::from_secs(20), CAPTURE_LIMIT).unwrap();
        assert_eq!(out.text, "ok\n1 dumb");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn capture_stops_at_the_timeout_with_what_it_has() {
        let dir = temp("timeout");
        let marker = dir.join("child-still-running");
        // The shell and the sleep it started both stop.
        let code = format!("echo before\n(sleep 3; touch '{}') &\nsleep 30\necho never", marker.display());
        let t = Instant::now();
        let out = capture(sh(), &code, &dir, Duration::from_millis(700), CAPTURE_LIMIT).unwrap();
        assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
        assert_eq!(out.text, "before\n[Strata stopped it after 0.7 s]");
        std::thread::sleep(Duration::from_millis(3500));
        assert!(!marker.exists(), "what it started in the background was stopped too");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn capture_keeps_the_start_of_huge_output() {
        let dir = temp("huge");
        let t = Instant::now();
        // About 59 MB, all read (the command isn't blocked), the first 64 KB kept.
        let out = capture(sh(), "i=0; yes 'a line of output to fill the pipe' | head -n 1400000; echo done-$((6*7))", &dir, Duration::from_secs(60), 64 * 1024).unwrap();
        assert!(t.elapsed() < Duration::from_secs(30), "{:?}", t.elapsed());
        let lines: Vec<&str> = out.text.lines().collect();
        assert!(lines.len() > 1000 && lines[..lines.len() - 1].iter().all(|l| *l == "a line of output to fill the pipe"), "only whole lines: {:?}", &lines[lines.len() - 3..]);
        assert!(out.text.len() <= 64 * 1024 + 200);
        let total = 1_400_000 * "a line of output to fill the pipe\n".len() + "done-42\n".len();
        assert_eq!(*lines.last().unwrap(), format!("[Strata kept the first 64 KB of {} of output]", size(total)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn capture_stops_background_jobs_when_the_block_ends() {
        let dir = temp("background");
        let t = Instant::now();
        let out = capture(sh(), "sleep 20 &\necho $! > bg.pid\necho started", &dir, Duration::from_secs(30), CAPTURE_LIMIT).unwrap();
        assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
        assert_eq!(out.text, "started");
        let pid = read_pid(&dir.join("bg.pid"));
        assert!(gone(pid), "the background sleep ({pid}) was stopped with the block");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_timeout_stops_children_in_a_group_of_their_own() {
        // `set -m` turns on job control: the background job gets its own
        // process group, out of reach of a signal to the block's group.
        let dir = temp("own-group");
        let out = capture(sh(), "set -m\nsleep 30 &\necho $! > own.pid\nsleep 30", &dir, Duration::from_millis(800), CAPTURE_LIMIT).unwrap();
        assert!(out.text.ends_with("[Strata stopped it after 0.8 s]"), "{}", out.text);
        let pid = read_pid(&dir.join("own.pid"));
        assert!(gone(pid), "the job in its own group ({pid}) was found and stopped");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_reused_process_id_is_never_signalled() {
        let mut child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        let pid = child.id() as i32;
        let s = tree::seen(pid).expect("a running child");
        assert_eq!((s.pid, s.ppid), (pid, std::process::id() as i32));
        assert!(tree::still(&s), "the same process");
        // The same id with another start time or parent is someone else.
        assert!(!tree::still(&tree::Seen { start: s.start + 1, ..s }), "started at another time");
        assert!(!tree::still(&tree::Seen { ppid: s.ppid + 1, ..s }), "another parent");
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(!tree::still(&s), "gone");
        assert!(tree::seen(pid).is_none_or(|now| now != s));
    }

    #[cfg(unix)]
    fn read_pid(path: &Path) -> i32 {
        std::fs::read_to_string(path).unwrap().trim().parse().unwrap()
    }

    /// `pid` is no longer running (waiting a moment for the kernel to reap it).
    #[cfg(unix)]
    fn gone(pid: i32) -> bool {
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            // SAFETY: signal 0 only checks that the process exists.
            if unsafe { libc::kill(pid, 0) } != 0 {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn hidden_and_reordering_characters_are_found() {
        for c in ['\u{202A}', '\u{202B}', '\u{202C}', '\u{202D}', '\u{202E}', '\u{2066}', '\u{2067}', '\u{2068}', '\u{2069}', '\u{200E}', '\u{200F}', '\u{061C}'] {
            assert_eq!(hidden(&format!("echo a{c}b")), vec![c], "bidi {:04X}", c as u32);
        }
        for c in ['\u{200B}', '\u{200C}', '\u{200D}', '\u{2060}', '\u{FEFF}', '\u{00AD}'] {
            assert_eq!(hidden(&format!("echo a{c}b")), vec![c], "invisible {:04X}", c as u32);
        }
        for c in ['\u{0}', '\u{1}', '\u{7}', '\u{8}', '\u{B}', '\u{C}', '\u{1B}', '\r', '\u{7F}', '\u{80}', '\u{85}', '\u{9B}', '\u{9F}'] {
            assert_eq!(hidden(&format!("echo a{c}b")), vec![c], "control {:04X}", c as u32);
        }
        assert!(hidden("echo \"tab\there\"\nls -l\n# café ünïcode 日本語 ✓").is_empty(), "tabs, line breaks and ordinary text are fine");
        assert_eq!(visible("rm -rf /tmp/x\u{202E}gnp.txt\u{200B}\r\techo"), "rm -rf /tmp/x⟨U+202E⟩gnp.txt⟨U+200B⟩⟨U+000D⟩\techo");
        assert_eq!(visible("plain\ntext"), "plain\ntext");
        // A CRLF file's blocks come through with plain line breaks; a lone CR stays and is refused.
        assert_eq!(code_blocks("```sh\r\necho a\r\necho b\r\n```\r\n", false)[0].code, "echo a\necho b");
        assert_eq!(hidden(&code_blocks("```sh\necho a\rb\n```\n", false)[0].code), vec!['\r']);
    }

    #[test]
    fn capture_runs_python_when_installed() {
        let Some(python) = lang("python") else { unreachable!() };
        if interpreter(python).is_err() {
            return;
        }
        let dir = temp("python");
        let out = capture(python, "import sys\nfor i in range(3):\n    print(i * 7)\nprint('oops', file=sys.stderr)\nsys.exit(2)", &dir, Duration::from_secs(30), CAPTURE_LIMIT).unwrap();
        assert_eq!(out.text, "0\n7\n14\noops\n[exit status 2]");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_interpreter_is_reported() {
        let fish_or_cmd = if cfg!(windows) { "fish" } else { "cmd" };
        let l = lang(fish_or_cmd).unwrap();
        if interpreter(l).is_ok() {
            return;
        }
        let err = capture(l, "echo", &std::env::temp_dir(), Duration::from_secs(5), CAPTURE_LIMIT).unwrap_err();
        assert!(err.contains("Couldn't find"), "{err}");
    }
}
