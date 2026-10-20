//! The built-in terminal panel. `alacritty_terminal` does the emulation and
//! owns the PTY (a Unix pseudo-terminal, or ConPTY on Windows); this module
//! paints its grid with egui, turns key presses into the bytes a shell
//! expects, and keeps several shells as tabs.

use crate::commands::Cmd;
use crate::keys::MAC;
use alacritty_terminal::event::{Event as TermEvent, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg, State};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::term::{Config, Osc52, Term, TermMode};
use alacritty_terminal::tty;
use alacritty_terminal::vte::ansi::{Color as AnsiColor, CursorShape, NamedColor, Rgb};
use egui::{Color32, Event, FontId, Key, Modifiers, Rect, Sense, Stroke, pos2, vec2};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

// ---- colors ------------------------------------------------------------------------

/// Terminal colors for the current editor theme.
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    pub bg: Color32,
    pub fg: Color32,
    pub cursor: Color32,
    pub selection: Color32,
    /// The 16 ANSI colors: 0–7 normal, 8–15 bright.
    pub ansi: [Color32; 16],
}

fn hex(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

impl Theme {
    /// Black and white come from the theme's own background and text so the
    /// terminal sits in the editor's palette; the hues are tuned for dark or
    /// light backgrounds.
    pub fn new(bg: Color32, fg: Color32, cursor: Color32, selection: Color32) -> Self {
        let light = (bg.r() as u32 + bg.g() as u32 + bg.b() as u32) > 3 * 140;
        let mut ansi = if light {
            [
                0, 0xd52f2f, 0x3b8f3a, 0xa86a00, 0x2f62d8, 0x9b2fae, 0x0b7d96, 0, //
                0, 0xe5484d, 0x46a345, 0xc08100, 0x4078f2, 0xb043c4, 0x1593b0, 0,
            ]
            .map(hex)
        } else {
            [
                0, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0, //
                0, 0xff7b86, 0xb5e890, 0xffd68a, 0x82c4ff, 0xde8ff0, 0x6fd7e3, 0,
            ]
            .map(hex)
        };
        if light {
            ansi[0] = mix(fg, Color32::BLACK, 0.2);
            ansi[7] = mix(bg, fg, 0.3);
            ansi[8] = mix(fg, bg, 0.45);
            ansi[15] = mix(bg, fg, 0.12);
        } else {
            ansi[0] = mix(bg, fg, 0.18);
            ansi[7] = mix(fg, Color32::WHITE, 0.1);
            ansi[8] = mix(bg, fg, 0.5);
            ansi[15] = mix(fg, Color32::WHITE, 0.7);
        }
        Self { bg, fg, cursor, selection, ansi }
    }

    /// Schemes with their own terminal palette (the curated ones) use its
    /// hues; bright variants are a step lighter (darker on light schemes).
    pub fn from_highlighter(hl: &crate::highlight::Highlighter) -> Self {
        let mut theme = Self::new(hl.background(), hl.foreground(), hl.caret(), hl.selection());
        if let Some(hues) = hl.ansi() {
            let toward = if hl.is_light() { Color32::BLACK } else { Color32::WHITE };
            for (i, c) in hues.into_iter().enumerate() {
                theme.ansi[i + 1] = c;
                theme.ansi[i + 9] = mix(c, toward, 0.18);
            }
        }
        theme
    }

    /// xterm's 256 colors: the 16 ANSI ones, a 6×6×6 cube, then a gray ramp.
    pub fn indexed(&self, i: u8) -> Color32 {
        match i {
            0..=15 => self.ansi[i as usize],
            16..=231 => {
                let n = i - 16;
                let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
                Color32::from_rgb(level(n / 36), level((n / 6) % 6), level(n % 6))
            }
            _ => {
                let v = 8 + (i - 232) * 10;
                Color32::from_rgb(v, v, v)
            }
        }
    }

    fn named(&self, c: NamedColor) -> Color32 {
        let n = c as usize;
        match c {
            NamedColor::Foreground | NamedColor::BrightForeground => self.fg,
            NamedColor::Background => self.bg,
            NamedColor::Cursor => self.cursor,
            NamedColor::DimForeground => mix(self.bg, self.fg, 0.66),
            _ if n < 16 => self.ansi[n],
            // Dim black … dim white follow Cursor.
            _ => mix(self.bg, self.ansi[n - NamedColor::DimBlack as usize], 0.66),
        }
    }

    /// A cell color, honoring palette changes the program made (OSC 4/10/11).
    pub fn resolve(&self, c: AnsiColor, overrides: &Colors) -> Color32 {
        let rgb = |c: Rgb| Color32::from_rgb(c.r, c.g, c.b);
        match c {
            AnsiColor::Spec(c) => rgb(c),
            AnsiColor::Indexed(i) => overrides[i as usize].map(rgb).unwrap_or_else(|| self.indexed(i)),
            AnsiColor::Named(n) => overrides[n].map(rgb).unwrap_or_else(|| self.named(n)),
        }
    }

    fn rgb_at(&self, index: usize) -> Rgb {
        let c = match index {
            0..=255 => self.indexed(index as u8),
            256 => self.fg,
            257 => self.bg,
            _ => self.cursor,
        };
        Rgb { r: c.r(), g: c.g(), b: c.b() }
    }
}

// ---- keys --------------------------------------------------------------------------

/// The character a key types, for Alt/Option-as-Meta (which sends ESC + char).
fn key_char(key: Key, shift: bool) -> Option<char> {
    let s = key.symbol_or_name();
    let mut chars = s.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return (key == Key::Space).then_some(' ');
    }
    if c.is_ascii_alphabetic() {
        return Some(if shift { c.to_ascii_uppercase() } else { c.to_ascii_lowercase() });
    }
    if !shift {
        return Some(c);
    }
    const PLAIN: &str = "0123456789-=[]\\;',./`";
    const SHIFTED: &str = ")!@#$%^&*(_+{}|:\"<>?~";
    Some(PLAIN.find(c).and_then(|i| SHIFTED[i..].chars().next()).unwrap_or(c))
}

/// Ctrl+key as a C0 control byte (Ctrl+A = 0x01 … the classic VT mapping).
fn ctrl_byte(key: Key) -> Option<u8> {
    let c = key.symbol_or_name();
    if c.len() == 1 && c.as_bytes()[0].is_ascii_alphabetic() {
        return Some(c.as_bytes()[0].to_ascii_uppercase() - b'A' + 1);
    }
    Some(match key {
        Key::Space | Key::Num2 | Key::Backtick => 0,
        Key::OpenBracket | Key::Num3 => 0x1b,
        Key::Backslash | Key::Num4 => 0x1c,
        Key::CloseBracket | Key::Num5 => 0x1d,
        Key::Num6 => 0x1e,
        Key::Minus | Key::Slash | Key::Num7 => 0x1f,
        Key::Num8 => 0x7f,
        _ => return None,
    })
}

/// The bytes a key press sends, or None when the key types text (that comes
/// as a Text event) or isn't for the terminal. `app_cursor` is DECCKM.
/// `alt_meta`: Alt/Option plus a character key sends Esc + the key's
/// character (Meta); otherwise the layout's composed text is typed instead
/// (Option+L is @ on a German Mac). Alt with Enter, Backspace, arrows and
/// other keys that type nothing always gets its Esc prefix or modifier code.
pub fn key_bytes(key: Key, m: Modifiers, app_cursor: bool, alt_meta: bool) -> Option<Vec<u8>> {
    if MAC && m.mac_cmd {
        return None;
    }
    let (shift, alt, ctrl) = (m.shift, m.alt, m.ctrl);
    // xterm's modifier parameter: 1 + Shift + 2·Alt + 4·Ctrl.
    let code = 1 + shift as u8 + 2 * alt as u8 + 4 * ctrl as u8;
    let cursor = |f: char| -> Vec<u8> {
        if code > 1 {
            format!("\x1b[1;{code}{f}")
        } else if app_cursor {
            format!("\x1bO{f}")
        } else {
            format!("\x1b[{f}")
        }
        .into_bytes()
    };
    let tilde = |n: u8| -> Vec<u8> { if code > 1 { format!("\x1b[{n};{code}~") } else { format!("\x1b[{n}~") }.into_bytes() };
    let ss3 = |f: char| -> Vec<u8> { if code > 1 { format!("\x1b[1;{code}{f}") } else { format!("\x1bO{f}") }.into_bytes() };
    let meta = |b: &[u8]| -> Vec<u8> {
        let mut v = if alt { vec![0x1b] } else { Vec::new() };
        v.extend_from_slice(b);
        v
    };
    Some(match key {
        Key::ArrowUp => cursor('A'),
        Key::ArrowDown => cursor('B'),
        Key::ArrowRight => cursor('C'),
        Key::ArrowLeft => cursor('D'),
        Key::Home => cursor('H'),
        Key::End => cursor('F'),
        Key::Insert => tilde(2),
        Key::Delete => tilde(3),
        Key::PageUp => tilde(5),
        Key::PageDown => tilde(6),
        Key::F1 => ss3('P'),
        Key::F2 => ss3('Q'),
        Key::F3 => ss3('R'),
        Key::F4 => ss3('S'),
        Key::F5 => tilde(15),
        Key::F6 => tilde(17),
        Key::F7 => tilde(18),
        Key::F8 => tilde(19),
        Key::F9 => tilde(20),
        Key::F10 => tilde(21),
        Key::F11 => tilde(23),
        Key::F12 => tilde(24),
        Key::Enter => meta(b"\r"),
        Key::Tab if shift => b"\x1b[Z".to_vec(),
        Key::Tab => meta(b"\t"),
        Key::Backspace => meta(if ctrl { b"\x08" } else { b"\x7f" }),
        Key::Escape => meta(b"\x1b"),
        _ if ctrl => meta(&[ctrl_byte(key)?]),
        _ if alt && alt_meta => meta(key_char(key, shift)?.to_string().as_bytes()),
        _ => return None,
    })
}

/// Text to send for a paste: bracketed when the program asked for it.
pub fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    // Pasted text never carries escape sequences: an ESC (or an 8-bit C1
    // control such as CSI, U+009B) would let copied text drive the terminal,
    // and with bracketed paste an embedded end marker would let the rest of
    // the text escape the bracket and run as typed input. Removing every ESC
    // and C1 control (rather than just the end marker, which a nested marker
    // like "\x1b[20\x1b[201~1~" would reconstitute) closes that for good.
    let clean: String = text.chars().filter(|&c| c != '\x1b' && !('\u{80}'..='\u{9f}').contains(&c)).collect();
    if bracketed {
        format!("\x1b[200~{clean}\x1b[201~").into_bytes()
    } else {
        clean.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}

/// Whether a window shortcut still works while the terminal has the keyboard.
/// The terminal owns plain, Ctrl and Alt keys; Cmd (macOS) or Ctrl+Shift
/// (elsewhere) combinations stay with Strata.
pub fn app_keeps(cmd: Option<Cmd>, mods: Modifiers) -> bool {
    if matches!(cmd, Some(Cmd::ToggleTerminal | Cmd::NewTerminal)) {
        return true;
    }
    if MAC { mods.mac_cmd || mods.command } else { (mods.ctrl || mods.command) && mods.shift }
}

/// Drop menu accelerators the terminal needs (macOS menus would swallow Ctrl+G and friends).
#[cfg(target_os = "macos")]
pub fn strip_menu_keys(items: &mut [crate::menus::Item]) {
    use crate::menus::{Act, Item};
    for it in items {
        match it {
            Item::Action { act, keys, .. } => {
                let cmd = if let Act::Cmd(c) = act { Some(*c) } else { None };
                if keys.is_some_and(|(m, _)| !app_keeps(cmd, m)) {
                    *keys = None;
                }
            }
            Item::Sub(_, kids) => strip_menu_keys(kids),
            _ => {}
        }
    }
}

// ---- one terminal ------------------------------------------------------------------

/// Forwards emulator events to the UI thread and wakes it up.
#[derive(Clone)]
struct Listener {
    ctx: Option<egui::Context>,
    queue: Arc<Mutex<Vec<TermEvent>>>,
    /// When the program last printed something (the emulator wakes the UI
    /// after each batch of output).
    output: Arc<Mutex<Option<std::time::Instant>>>,
}

impl EventListener for Listener {
    fn send_event(&self, event: TermEvent) {
        match event {
            TermEvent::Wakeup => *self.output.lock().unwrap_or_else(|e| e.into_inner()) = Some(std::time::Instant::now()),
            TermEvent::MouseCursorDirty | TermEvent::CursorBlinkingChange => {}
            e => self.queue.lock().unwrap_or_else(|e| e.into_inner()).push(e),
        }
        if let Some(ctx) = &self.ctx {
            ctx.request_repaint();
        }
    }
}

#[derive(Clone, Copy)]
struct Size {
    cols: usize,
    rows: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

type Loop = EventLoop<tty::Pty, Listener>;

pub struct Terminal {
    term: Arc<FairMutex<Term<Listener>>>,
    sender: EventLoopSender,
    queue: Arc<Mutex<Vec<TermEvent>>>,
    thread: Option<JoinHandle<(Loop, State)>>,
    /// Shell process id (also its process group: the PTY child calls setsid).
    #[cfg(unix)]
    pid: u32,
    title: Option<String>,
    shell_name: String,
    exited: bool,
    size: Size,
    cell: (f32, f32),
    scroll_acc: f32,
    bell: f64,
    focused: bool,
    /// Text the input method is composing (shown at the cursor, not yet sent).
    preedit: String,
    /// Mouse buttons pressed inside the terminal and reported to the program
    /// (left, middle, right), and the cell last reported, for drag/motion.
    mouse_down: [bool; 3],
    mouse_cell: Option<(usize, usize)>,
    /// The PTY (a second handle), to ask which program is in the foreground.
    #[cfg(unix)]
    tty: Option<std::fs::File>,
    /// The folder the shell started in.
    cwd: PathBuf,
    started: std::time::Instant,
    /// When the program last printed something.
    last_output: Arc<Mutex<Option<std::time::Instant>>>,
    /// Send to Terminal text waiting for a shell that's still starting (or
    /// for the Enter that closes the previous text's block).
    pending_send: Vec<String>,
    /// When to press the Enter that closes a sent Python block, if Python
    /// is still waiting for it then.
    enter_after: Option<std::time::Instant>,
    /// The file location under the pointer while ⌘ (Ctrl) is held.
    link: Option<Link>,
    /// What link lookups remember while the modifier is held.
    lookup: LinkLookup,
    /// Checks link candidates on disk (started on first use).
    resolver: Option<Resolver>,
    /// Which press of the modifier lookups belong to (answers to an earlier
    /// one are dropped).
    generation: u64,
    /// The shell's folder as the kernel last gave it, and when.
    shell_dir: Option<(std::time::Instant, Option<PathBuf>)>,
    /// The side bar folders lookups last used, and a number that changes with them.
    roots_seen: Vec<PathBuf>,
    roots_generation: u64,
    /// Distinct for every terminal started, so a destination chosen earlier
    /// (Run Code Block's confirmation) can't land in a different tab.
    id: u64,
    /// The REPL this tab was started for by Run Code Block (`python3`, …).
    repl: Option<String>,
}

/// What finding links remembers on the UI thread while the modifier is held;
/// emptied when it's released.
#[derive(Default)]
struct LinkLookup {
    /// The last logical line searched, and the locations in it.
    line: Option<(String, Vec<crate::build::TextLocation>)>,
    /// What the resolver said files are, and when it said so (answers older
    /// than `FILE_CHECK_TTL` are asked again, as files come and go).
    answers: HashMap<AnswerKey, (std::time::Instant, Option<PathBuf>)>,
    /// What the resolver has been asked, and when.
    asked: HashMap<AnswerKey, std::time::Instant>,
}

/// What an answer depends on: the shell's folder as the kernel gave it
/// (after a `cd` the same printed path names another file), the side bar
/// folders (by `Terminal::roots_generation`), and the path as printed.
type AnswerKey = (Option<PathBuf>, u64, String);

/// A link candidate for the resolver.
struct Resolve {
    generation: u64,
    key: AnswerKey,
    /// Where the shell started, and the side bar folders.
    cwd: PathBuf,
    roots: Vec<PathBuf>,
}

/// Checks link candidates on disk on its own thread, so a slow network
/// folder or a drive that's gone can't freeze the editor; the underline
/// appears when the answer does.
struct Resolver {
    ask: std::sync::mpsc::Sender<Resolve>,
    answers: std::sync::mpsc::Receiver<(u64, AnswerKey, Option<PathBuf>)>,
}

impl Resolver {
    fn start(ctx: egui::Context) -> Option<Self> {
        let (ask, questions) = std::sync::mpsc::channel::<Resolve>();
        let (tell, answers) = std::sync::mpsc::channel();
        let run = move || {
            let mut disk = DiskLookup::default();
            // Ends when the terminal (and so `ask`) goes away. A check stuck
            // on a dead network mount keeps the thread (only) until the
            // system gives up on it; the terminal doesn't wait for it.
            while let Ok(r) = questions.recv() {
                let (shell_dir, _, file) = &r.key;
                let mut bases = vec![disk.spell_dir(shell_dir.as_deref(), &r.cwd, &r.roots), r.cwd.clone()];
                bases.dedup();
                bases.extend(r.roots);
                let path = resolve_file(file, &bases, |p| disk.is_file(p));
                if tell.send((r.generation, r.key, path)).is_err() {
                    break;
                }
                ctx.request_repaint();
            }
        };
        std::thread::Builder::new().name("terminal-links".into()).spawn(run).ok()?;
        Some(Self { ask, answers })
    }
}

/// What the resolver remembers, so holding ⌘ (Ctrl) over output that keeps
/// changing (`cargo watch`, `tail -f`) doesn't stat files for every new line.
#[derive(Default)]
struct DiskLookup {
    /// Where the shell started and the side bar folders, each with its
    /// canonical path (for spelling the shell's folder their way).
    canon: Vec<(PathBuf, Option<PathBuf>)>,
    /// Whether paths are files, and when that was checked.
    files: HashMap<PathBuf, (std::time::Instant, bool)>,
}

/// How long the shell's folder and a file check are trusted while the modifier is held.
const SHELL_DIR_TTL: std::time::Duration = std::time::Duration::from_millis(500);
const FILE_CHECK_TTL: std::time::Duration = std::time::Duration::from_secs(2);
/// Locations looked at per logical line (a huge line can hold thousands of `a:1`).
const MAX_LOCATIONS_PER_LINE: usize = 64;
/// File checks remembered before starting over.
const MAX_FILE_CHECKS: usize = 512;

impl DiskLookup {
    /// `p.is_file()`, remembered for `FILE_CHECK_TTL`.
    fn is_file(&mut self, p: &Path) -> bool {
        if let Some(&(at, yes)) = self.files.get(p)
            && at.elapsed() < FILE_CHECK_TTL
        {
            return yes;
        }
        if self.files.len() >= MAX_FILE_CHECKS {
            self.files.clear();
        }
        #[cfg(test)]
        record_disk_check(p);
        let yes = p.is_file();
        self.files.insert(p.to_owned(), (std::time::Instant::now(), yes));
        yes
    }

    /// The shell's folder `dir` as the kernel gave it (else where it
    /// started, `cwd`). The kernel resolves symbolic links (`/private/var`
    /// for `/var` on macOS); inside where the shell started or one of
    /// `roots` it's spelled their way, so opened files match the tabs and
    /// side bar. Their canonical paths are kept until they change.
    fn spell_dir(&mut self, dir: Option<&Path>, cwd: &Path, roots: &[PathBuf]) -> PathBuf {
        if let Some(dir) = dir {
            let bases: Vec<&Path> = std::iter::once(cwd).chain(roots.iter().map(PathBuf::as_path)).collect();
            if self.canon.iter().map(|(b, _)| b.as_path()).ne(bases.iter().copied()) {
                self.canon = bases
                    .into_iter()
                    .map(|b| {
                        #[cfg(test)]
                        record_disk_check(b);
                        (b.to_owned(), b.canonicalize().ok())
                    })
                    .collect();
            }
            for (base, canon) in &self.canon {
                if let Some(rest) = canon.as_ref().and_then(|c| dir.strip_prefix(c).ok()) {
                    return if rest.as_os_str().is_empty() { base.clone() } else { base.join(rest) };
                }
            }
            return dir.to_owned();
        }
        cwd.to_owned()
    }
}

/// Paths link lookups checked on disk, and on which thread (tests make sure
/// it's never the UI thread).
#[cfg(test)]
static DISK_CHECKS: Mutex<Vec<(PathBuf, std::thread::ThreadId)>> = Mutex::new(Vec::new());

#[cfg(test)]
fn record_disk_check(p: &Path) {
    DISK_CHECKS.lock().unwrap_or_else(|e| e.into_inner()).push((p.to_owned(), std::thread::current().id()));
}

/// Part of a link on one row: (grid line, first column, column after the last).
type LinkRow = (Line, usize, usize);

/// A clickable file location in the output.
#[derive(Debug)]
struct Link {
    /// The cells it covers, a row at a time.
    rows: Vec<LinkRow>,
    path: PathBuf,
    line: usize,
    col: usize,
}

/// How a terminal tab runs a REPL: through the user's login shell (as a new
/// tab's shell starts), so it finds the same `python3` or `node` the user's
/// own terminal would. On Windows, the program itself.
pub fn repl_command(program: &str) -> (String, Vec<String>) {
    if cfg!(windows) {
        return (program.to_owned(), Vec::new());
    }
    let (shell, mut args) = default_shell();
    args.extend(["-c".to_owned(), format!("exec {program}")]);
    (shell, args)
}

/// The user's shell: $SHELL, else zsh, else sh (PowerShell, else COMSPEC, on Windows).
fn default_shell() -> (String, Vec<String>) {
    if cfg!(windows) {
        return ("powershell.exe".into(), vec!["-NoLogo".into()]);
    }
    let shell = std::env::var("SHELL")
        .ok()
        .filter(|s| !s.is_empty() && Path::new(s).exists())
        .or_else(|| ["/bin/zsh", "/bin/bash", "/bin/sh"].into_iter().find(|s| Path::new(s).exists()).map(String::from))
        .unwrap_or_else(|| "/bin/sh".into());
    // macOS terminals start login shells (so /etc/zprofile sets PATH).
    let args = if MAC { vec!["-l".into()] } else { Vec::new() };
    (shell, args)
}

impl Terminal {
    /// Start `shell` (default: the user's login shell) in `cwd`.
    pub fn spawn(ctx: Option<egui::Context>, cwd: &Path, shell: Option<(String, Vec<String>)>, cols: usize, rows: usize) -> std::io::Result<Self> {
        let explicit = shell.is_some();
        let (program, args) = shell.unwrap_or_else(default_shell);
        let env: HashMap<String, String> = [("TERM", "xterm-256color"), ("COLORTERM", "truecolor"), ("TERM_PROGRAM", "Strata")]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect();
        #[allow(clippy::needless_update)] // Windows has more fields
        let mut options = tty::Options {
            shell: Some(tty::Shell::new(program.clone(), args)),
            working_directory: Some(cwd.to_owned()),
            drain_on_exit: true,
            env,
            ..Default::default()
        };
        let size = Size { cols: cols.max(2), rows: rows.max(1) };
        let window = WindowSize { num_lines: size.rows as u16, num_cols: size.cols as u16, cell_width: 8, cell_height: 16 };
        let pty = match tty::new(&options, window, 0) {
            Ok(p) => p,
            Err(e) if cfg!(windows) && !explicit => {
                // No PowerShell: fall back to cmd.exe.
                let _ = e;
                let comspec = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into());
                options.shell = Some(tty::Shell::new(comspec, Vec::new()));
                tty::new(&options, window, 0)?
            }
            Err(e) => return Err(e),
        };
        #[cfg(unix)]
        let pid = pty.child().id();
        #[cfg(unix)]
        let tty = pty.file().try_clone().ok();
        let queue = Arc::new(Mutex::new(Vec::new()));
        let last_output = Arc::new(Mutex::new(None));
        let listener = Listener { ctx, queue: queue.clone(), output: last_output.clone() };
        // Clipboard reads reach `poll`, which refuses them unless the setting
        // allows (and says so); the emulator's own default drops them silently.
        let config = Config { osc52: Osc52::CopyPaste, ..Config::default() };
        let term = Arc::new(FairMutex::new(Term::new(config, &size, listener.clone())));
        let event_loop = EventLoop::new(term.clone(), listener, pty, true, false)?;
        let sender = event_loop.channel();
        let thread = Some(event_loop.spawn());
        let shell_name = Path::new(&program).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or(program);
        static IDS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let id = IDS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(Self { term, sender, queue, thread,
            #[cfg(unix)]
            pid,
            title: None, shell_name, exited: false, size, cell: (8.0, 16.0), scroll_acc: 0.0, bell: f64::NEG_INFINITY, focused: false,
            preedit: String::new(), mouse_down: [false; 3], mouse_cell: None,
            #[cfg(unix)]
            tty,
            cwd: cwd.to_owned(), started: std::time::Instant::now(), last_output, pending_send: Vec::new(), enter_after: None, link: None,
            lookup: LinkLookup::default(), resolver: None, generation: 0, shell_dir: None, roots_seen: Vec::new(), roots_generation: 0, id, repl: None })
    }

    pub fn title(&self) -> &str {
        // A REPL tab runs through the login shell, so its shell's name would mislead.
        self.title.as_deref().filter(|t| !t.trim().is_empty()).or(self.repl.as_deref()).unwrap_or(&self.shell_name)
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    /// The REPL Run Code Block started this tab for, if it did.
    pub fn repl(&self) -> Option<&str> {
        self.repl.as_deref()
    }

    pub fn write(&self, bytes: impl Into<Vec<u8>>) {
        let bytes: Vec<u8> = bytes.into();
        if !bytes.is_empty() {
            let _ = self.sender.send(Msg::Input(bytes.into()));
        }
    }

    /// Typing jumps back to the live screen and drops the selection, like other terminals.
    fn type_bytes(&self, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        {
            let mut t = self.term.lock();
            t.scroll_display(Scroll::Bottom);
            t.selection = None;
        }
        self.write(bytes);
    }

    fn resize(&mut self, cols: usize, rows: usize, cell: (f32, f32)) {
        let size = Size { cols: cols.max(2), rows: rows.max(1) };
        if size.cols == self.size.cols && size.rows == self.size.rows && cell == self.cell {
            return;
        }
        self.size = size;
        self.cell = cell;
        self.term.lock().resize(size);
        let window = WindowSize { num_lines: size.rows as u16, num_cols: size.cols as u16, cell_width: cell.0 as u16, cell_height: cell.1 as u16 };
        let _ = self.sender.send(Msg::Resize(window));
    }

    /// Handle what the emulator reported since the last frame, reporting
    /// clipboard use (OSC 52) in `clip`.
    fn poll(&mut self, ctx: &egui::Context, theme: &Theme, allow: Clipboard, clip: &mut ClipboardUse) {
        let events = std::mem::take(&mut *self.queue.lock().unwrap_or_else(|e| e.into_inner()));
        for e in events {
            match e {
                TermEvent::Title(t) => self.title = Some(t),
                TermEvent::ResetTitle => self.title = None,
                // Writing the clipboard (OSC 52) is allowed by default, as in
                // other terminals, and always reported (the app says so in the
                // status bar); reading it hands whatever you copied (a
                // password, say) to any program that prints the right escape
                // sequence, so it's off unless allowed.
                TermEvent::ClipboardStore(_, text) if allow.write => {
                    ctx.copy_text(text);
                    clip.wrote = true;
                }
                TermEvent::ClipboardStore(..) => clip.write_blocked = true,
                TermEvent::ClipboardLoad(_, format) if allow.read => {
                    if let Ok(text) = arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
                        self.write(format(&text));
                    }
                }
                TermEvent::ClipboardLoad(..) => clip.read_blocked = true,
                TermEvent::ColorRequest(i, format) => self.write(format(theme.rgb_at(i))),
                TermEvent::TextAreaSizeRequest(format) => {
                    let (w, h) = self.cell;
                    self.write(format(WindowSize { num_lines: self.size.rows as u16, num_cols: self.size.cols as u16, cell_width: w as u16, cell_height: h as u16 }));
                }
                TermEvent::PtyWrite(text) => self.write(text),
                TermEvent::Bell => {
                    self.bell = ctx.input(|i| i.time);
                    ctx.request_repaint_after(std::time::Duration::from_millis(200));
                }
                TermEvent::ChildExit(_) | TermEvent::Exit => self.exited = true,
                TermEvent::Wakeup | TermEvent::MouseCursorDirty | TermEvent::CursorBlinkingChange => {}
            }
        }
        if let Some(at) = self.enter_after {
            let now = std::time::Instant::now();
            if now < at {
                ctx.request_repaint_after(at - now);
            } else if self.python_waiting_for_block_end() {
                self.enter_after = None;
                self.type_bytes(b"\r".to_vec());
            } else if now - at > BLOCK_ENTER_PATIENCE {
                // Not Python, or it didn't ask for more: no Enter, which
                // could answer a password or confirmation prompt.
                self.enter_after = None;
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
            }
        }
        while self.enter_after.is_none() && !self.pending_send.is_empty() && self.ready_for_input() {
            let text = self.pending_send.remove(0);
            self.send_now(&text);
        }
        if !self.pending_send.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    /// The shell's current folder as the kernel gives it (after any `cd`),
    /// asked at most every `SHELL_DIR_TTL`. This asks the kernel about the
    /// process, not the disk. `None` on Windows.
    fn raw_shell_dir(&mut self) -> Option<PathBuf> {
        if let Some((at, dir)) = &self.shell_dir
            && at.elapsed() < SHELL_DIR_TTL
        {
            return dir.clone();
        }
        #[cfg(unix)]
        let dir = process_dir(self.pid);
        #[cfg(not(unix))]
        let dir = None;
        self.shell_dir = Some((std::time::Instant::now(), dir.clone()));
        dir
    }

    /// A new shell is ready for sent text when its line editor turns on
    /// bracketed paste, or once it has printed something (its prompt) and
    /// gone quiet. Sooner, startup files (conda, nvm) can swallow the text,
    /// or the PTY echoes it before the line editor starts.
    fn ready_for_input(&self) -> bool {
        if self.started.elapsed() >= SHELL_START_CAP || self.term.lock().mode().contains(TermMode::BRACKETED_PASTE) {
            return true;
        }
        self.last_output.lock().unwrap_or_else(|e| e.into_inner()).is_some_and(|t| t.elapsed() >= SHELL_QUIET)
    }

    /// Python's REPL is in the foreground at its "..." prompt, waiting for an
    /// empty line to end a block. Both must hold: anything else (a shell,
    /// `sudo`, `ssh`, a debugger) gets no extra Enter.
    fn python_waiting_for_block_end(&self) -> bool {
        let at_continuation = {
            let term = self.term.lock();
            let grid = term.grid();
            let line = grid.cursor.point.line;
            let row: String = (0..grid.columns()).map(|c| grid[line][Column(c)].c).collect();
            row.trim() == "..."
        };
        at_continuation && self.foreground_is_python()
    }

    /// The program in the foreground of the terminal is Python, by the name
    /// of its process group's leader.
    #[cfg(unix)]
    fn foreground_is_python(&self) -> bool {
        use std::os::fd::AsRawFd;
        unsafe extern "C" {
            fn tcgetpgrp(fd: i32) -> i32;
        }
        let Some(tty) = &self.tty else { return false };
        let group = unsafe { tcgetpgrp(tty.as_raw_fd()) };
        group > 0 && process_name(group as u32).is_some_and(|n| is_python(&n))
    }

    /// Windows can't say which program is in the foreground of a console.
    #[cfg(not(unix))]
    fn foreground_is_python(&self) -> bool {
        false
    }

    /// Send to Terminal: `text` as a paste, then Enter, so a multi-line
    /// selection runs as one input in shells and REPLs that bracket pastes.
    pub fn send(&mut self, text: &str) {
        if !self.pending_send.is_empty() || self.enter_after.is_some() || !self.ready_for_input() {
            self.pending_send.push(text.to_owned());
        } else {
            self.send_now(text);
        }
    }

    fn send_now(&mut self, text: &str) {
        let text = text.trim_end_matches(['\r', '\n']);
        if !text.is_empty() {
            self.paste(text);
        }
        self.type_bytes(b"\r".to_vec());
        // A Python block whose last line is indented (a `for` loop's body)
        // stays open at "..." until an empty line ends it. The Enter for that
        // goes a moment later (Python's own REPL takes keys that arrive
        // together as a paste, where Enter only starts a new line), and only
        // if Python is then waiting for it.
        if open_block(text) {
            self.enter_after = Some(std::time::Instant::now() + BLOCK_ENTER_DELAY);
        }
    }


    /// The visible screen as text, one line per row.
    #[cfg(all(test, unix))]
    pub fn screen_text(&self) -> String {
        let t = self.term.lock();
        let grid = t.grid();
        let mut out = String::new();
        for line in 0..grid.screen_lines() {
            let row: String = (0..grid.columns())
                .map(|c| &grid[Point::new(Line(line as i32), Column(c))])
                .filter(|cell| !cell.flags.contains(Flags::WIDE_CHAR_SPACER))
                .map(|cell| cell.c)
                .collect();
            out.push_str(row.trim_end());
            out.push('\n');
        }
        out
    }

    /// An input-method event (Chinese, Japanese, Korean…): track the
    /// composition, and return committed text for the shell.
    fn ime(&mut self, ime: egui::ImeEvent) -> Option<String> {
        match ime {
            egui::ImeEvent::Preedit { text, .. } if text != "\n" && text != "\r" => {
                self.preedit = text;
                None
            }
            egui::ImeEvent::Commit(text) => {
                self.preedit.clear();
                // A committed newline also arrives as an Enter key press.
                (!text.is_empty() && text != "\n" && text != "\r").then_some(text)
            }
            #[allow(deprecated)]
            egui::ImeEvent::Enabled | egui::ImeEvent::Disabled => {
                self.preedit.clear();
                None
            }
            _ => None,
        }
    }

    fn selection_text(&self) -> Option<String> {
        self.term.lock().selection_to_string().filter(|s| !s.is_empty())
    }

    fn paste(&self, text: &str) {
        let bracketed = self.term.lock().mode().contains(TermMode::BRACKETED_PASTE);
        self.type_bytes(paste_bytes(text, bracketed));
    }

    /// Cmd+K: drop the scrollback and let the shell redraw its prompt at the top.
    fn clear(&self) {
        let mut t = self.term.lock();
        if t.mode().contains(TermMode::ALT_SCREEN) {
            return;
        }
        t.grid_mut().clear_history();
        t.selection = None;
        drop(t);
        self.write(vec![0x0c]);
    }
}

/// The last non-blank line is indented: a Python block is still open.
fn open_block(text: &str) -> bool {
    text.lines().rev().find(|l| !l.trim().is_empty()).is_some_and(|l| l.starts_with([' ', '\t']))
}

/// How long after sending an open block its closing Enter follows, and how
/// much longer Python may take to show its "..." prompt.
const BLOCK_ENTER_DELAY: std::time::Duration = std::time::Duration::from_millis(200);
const BLOCK_ENTER_PATIENCE: std::time::Duration = std::time::Duration::from_millis(1000);

/// A new shell that printed something and then nothing for this long is at its prompt.
const SHELL_QUIET: std::time::Duration = std::time::Duration::from_millis(300);
/// The longest Send to Terminal waits for a new shell.
const SHELL_START_CAP: std::time::Duration = std::time::Duration::from_secs(10);

/// A Python interpreter's process name: python or pypy, with an optional
/// version (python3, python3.14, pypy3). Helpers such as python-agent don't
/// count. On macOS, where the framework build runs as "Python", case is
/// ignored.
#[cfg(unix)]
fn is_python(name: &str) -> bool {
    let name = if MAC { name.to_ascii_lowercase() } else { name.to_owned() };
    let Some(version) = name.strip_prefix("python").or_else(|| name.strip_prefix("pypy")) else { return false };
    version.is_empty() || version.split('.').all(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

#[cfg(target_os = "macos")]
fn process_name(pid: u32) -> Option<String> {
    unsafe extern "C" {
        fn proc_name(pid: i32, buffer: *mut std::ffi::c_void, size: u32) -> i32;
    }
    let mut buf = [0u8; 256];
    let n = unsafe { proc_name(pid as i32, buf.as_mut_ptr().cast(), buf.len() as u32) };
    (n > 0).then(|| String::from_utf8_lossy(&buf[..n as usize]).into_owned())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn process_name(pid: u32) -> Option<String> {
    std::fs::read_to_string(format!("/proc/{pid}/comm")).ok().map(|s| s.trim_end().to_owned())
}

/// A process's current folder, from the kernel: shells don't report `cd`
/// unless configured to (OSC 7), and the emulator ignores that anyway.
#[cfg(target_os = "macos")]
fn process_dir(pid: u32) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    unsafe extern "C" {
        fn proc_pidinfo(pid: i32, flavor: i32, arg: u64, buffer: *mut std::ffi::c_void, size: i32) -> i32;
    }
    const PROC_PIDVNODEPATHINFO: i32 = 9;
    // struct proc_vnodepathinfo: the current and root folders, each a
    // vnode_info (152 bytes) followed by a MAXPATHLEN (1024) path.
    const VNODE_INFO: usize = 152;
    let mut buf = vec![0u8; 2 * (VNODE_INFO + 1024)];
    let n = unsafe { proc_pidinfo(pid as i32, PROC_PIDVNODEPATHINFO, 0, buf.as_mut_ptr().cast(), buf.len() as i32) };
    if n as usize != buf.len() {
        return None;
    }
    let path = &buf[VNODE_INFO..VNODE_INFO + 1024];
    let end = path.iter().position(|&b| b == 0)?;
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(&path[..end]))).filter(|p| p.is_absolute())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn process_dir(pid: u32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
}

#[cfg(unix)]
fn hang_up(pid: u32) {
    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    const SIGHUP: i32 = 1;
    // The whole process group: the shell and the jobs it started. And the
    // shell itself, in case it hasn't made its own group yet (closed right
    // after opening).
    unsafe {
        kill(-(pid as i32), SIGHUP);
        kill(pid as i32, SIGHUP);
    }
}

#[cfg(unix)]
fn force_kill(pid: u32) {
    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    const SIGKILL: i32 = 9;
    unsafe {
        kill(-(pid as i32), SIGKILL);
        kill(pid as i32, SIGKILL);
    }
}

#[cfg(all(test, unix))]
impl Terminal {
    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub fn preedit(&self) -> &str {
        &self.preedit
    }

    pub fn cell_size(&self) -> (f32, f32) {
        self.cell
    }

    /// Columns and rows of the grid.
    pub fn grid_size(&self) -> (usize, usize) {
        (self.size.cols, self.size.rows)
    }

    /// The underlined location: its file, line, column, and the cells it covers.
    #[cfg(test)]
    pub fn hovered_link(&self) -> Option<(&Path, usize, usize, &[LinkRow])> {
        self.link.as_ref().map(|l| (l.path.as_path(), l.line, l.col, l.rows.as_slice()))
    }

    /// Send to Terminal text is still waiting for the shell.
    #[cfg(test)]
    pub fn has_pending_send(&self) -> bool {
        !self.pending_send.is_empty()
    }

    /// The paths link lookups checked on disk from thread `thread`.
    #[cfg(test)]
    pub fn disk_checks_on(thread: std::thread::ThreadId) -> Vec<PathBuf> {
        DISK_CHECKS.lock().unwrap_or_else(|e| e.into_inner()).iter().filter(|(_, t)| *t == thread).map(|(p, _)| p.clone()).collect()
    }

    /// Link lookups have remembered nothing (so done no disk work).
    #[cfg(test)]
    pub fn link_lookup_is_empty(&self) -> bool {
        let l = &self.lookup;
        l.line.is_none() && l.answers.is_empty() && l.asked.is_empty() && self.resolver.is_none()
    }
}

impl Drop for Terminal {
    /// Hang up the shell and reap it off the UI thread, so closing never blocks.
    fn drop(&mut self) {
        let _ = self.sender.send(Msg::Shutdown);
        #[cfg(unix)]
        if !self.exited {
            hang_up(self.pid);
        }
        let Some(thread) = self.thread.take() else { return };
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        #[cfg(unix)]
        {
            let pid = self.pid;
            // A shell that ignores SIGHUP gets SIGKILL. Until the reaper's wait
            // returns, the pid can't be reused, so this can't hit a stranger.
            let done = done.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(2));
                if !done.load(std::sync::atomic::Ordering::SeqCst) {
                    force_kill(pid);
                }
            });
        }
        // Dropping the event loop drops the PTY, which waits for the child.
        std::thread::spawn(move || {
            drop(thread.join());
            done.store(true, std::sync::atomic::Ordering::SeqCst);
        });
    }
}

// ---- the panel ---------------------------------------------------------------------

/// What the panel asks the app to do.
pub enum PanelAction {
    Hide,
    /// The panel needs a new shell (the "+" button).
    NewTab,
    /// Open a file at a 0-based line and column (a ⌘- or Ctrl-clicked location).
    Open(PathBuf, usize, usize),
}

pub struct Panel {
    pub tabs: Vec<Terminal>,
    pub active: usize,
    pub visible: bool,
    focus_next: bool,
    id: egui::Id,
    theme: Option<Theme>,
    /// Background opacity of the text area (the window opacity setting); the tab row stays solid.
    pub opacity: f32,
    /// This frame's text area and tab labels (tab index, response), for context menus.
    pub body: Option<egui::Response>,
    pub tab_labels: Vec<(usize, egui::Response)>,
    /// Alt/Option sends Esc + key instead of the layout's character (`terminal_option_as_meta`).
    pub option_as_meta: bool,
    /// What programs may do with the clipboard through OSC 52.
    pub clipboard: Clipboard,
    /// What they did or tried since the app last looked (it clears this).
    pub clipboard_use: ClipboardUse,
    /// The side bar folders, where relative paths in the output are looked
    /// for when they aren't in the shell's folder.
    pub roots: Vec<PathBuf>,
    /// A REPL tab that ended before its text was sent (the program isn't
    /// installed, say), for the app to report; it takes it.
    pub repl_failed: Option<String>,
}

/// OSC 52 permissions (`terminal_allow_clipboard_read` / `_write`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Clipboard {
    pub read: bool,
    pub write: bool,
}

/// OSC 52 activity since the app last took it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ClipboardUse {
    /// A program set the clipboard.
    pub wrote: bool,
    /// A program tried to set it and `write` refused.
    pub write_blocked: bool,
    /// A program asked to read it and `read` refused.
    pub read_blocked: bool,
}

impl Default for Panel {
    fn default() -> Self {
        Self::new()
    }
}

/// One painted cell.
#[derive(Clone)]
struct Cell {
    c: char,
    extra: Option<Box<str>>,
    fg: Color32,
    bg: Color32,
    flags: Flags,
}

impl Panel {
    pub fn new() -> Self {
        Self {
            opacity: 1.0,
            tabs: Vec::new(),
            active: 0,
            visible: false,
            focus_next: false,
            id: egui::Id::new("strata-terminal"),
            theme: None,
            body: None,
            tab_labels: Vec::new(),
            clipboard: Clipboard { read: false, write: true },
            clipboard_use: ClipboardUse::default(),
            option_as_meta: !MAC,
            roots: Vec::new(),
            repl_failed: None,
        }
    }

    pub fn has_focus(&self, ctx: &egui::Context) -> bool {
        self.visible && !self.tabs.is_empty() && ctx.memory(|m| m.has_focus(self.id))
    }

    /// Give the terminal the keyboard on the next frame.
    pub fn focus(&mut self) {
        self.focus_next = true;
    }

    pub fn open(&mut self, ctx: &egui::Context, cwd: &Path) -> Result<(), String> {
        let (cols, rows) = self.tabs.get(self.active).map(|t| (t.size.cols, t.size.rows)).unwrap_or((80, 12));
        let t = Terminal::spawn(Some(ctx.clone()), cwd, None, cols, rows).map_err(|e| format!("Terminal: {e}"))?;
        self.tabs.push(t);
        self.active = self.tabs.len() - 1;
        self.visible = true;
        Ok(())
    }

    /// A new tab running `program`'s REPL (see `repl_command`), made active.
    pub fn open_repl(&mut self, ctx: &egui::Context, cwd: &Path, program: &str) -> Result<(), String> {
        let (cols, rows) = self.tabs.get(self.active).map(|t| (t.size.cols, t.size.rows)).unwrap_or((80, 12));
        let mut t = Terminal::spawn(Some(ctx.clone()), cwd, Some(repl_command(program)), cols, rows).map_err(|e| format!("Terminal: {e}"))?;
        t.repl = Some(program.to_owned());
        self.tabs.push(t);
        self.active = self.tabs.len() - 1;
        self.visible = true;
        Ok(())
    }

    /// The tab of the terminal with this `Terminal::id`, if it's still open.
    pub fn index_of(&self, id: u64) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }

    pub fn active_terminal(&self) -> Option<&Terminal> {
        self.tabs.get(self.active)
    }

    pub fn active_terminal_mut(&mut self) -> Option<&mut Terminal> {
        self.tabs.get_mut(self.active)
    }

    /// The active terminal has selected text.
    pub fn has_selection(&self) -> bool {
        self.active_terminal().is_some_and(|t| t.selection_text().is_some())
    }

    /// Copy the active terminal's selection (kept selected). False if none.
    pub fn copy(&self, ctx: &egui::Context) -> bool {
        match self.active_terminal().and_then(Terminal::selection_text) {
            Some(s) => {
                ctx.copy_text(s);
                true
            }
            None => false,
        }
    }

    /// Paste into the active shell (bracketed when the shell asks for it).
    pub fn paste(&self, text: &str) {
        if let Some(t) = self.active_terminal() {
            t.paste(text);
        }
    }

    /// Select the active terminal's whole scrollback and screen.
    pub fn select_all(&self) {
        let Some(t) = self.active_terminal() else { return };
        let mut term = t.term.lock();
        let (top, bottom, last) = (term.grid().topmost_line(), term.grid().bottommost_line(), term.grid().last_column());
        let mut sel = Selection::new(SelectionType::Simple, Point::new(top, Column(0)), Side::Left);
        sel.update(Point::new(bottom, last), Side::Right);
        term.selection = Some(sel);
    }

    /// Clear the active terminal's scrollback (Cmd+K).
    pub fn clear(&self) {
        if let Some(t) = self.active_terminal() {
            t.clear();
        }
    }

    /// Close terminal `i`, hanging up its shell. True if none are left.
    pub fn close(&mut self, i: usize) -> bool {
        if i < self.tabs.len() {
            self.tabs.remove(i);
            if self.active > i || self.active >= self.tabs.len() {
                self.active = self.active.saturating_sub(1);
            }
        }
        self.tabs.is_empty()
    }

    /// Process emulator events; close tabs whose shell exited. True when the
    /// last one just closed (the panel hides itself).
    pub fn poll(&mut self, ctx: &egui::Context) -> bool {
        if self.tabs.is_empty() {
            return false;
        }
        let theme = self.theme.clone().unwrap_or_else(|| Theme::new(Color32::BLACK, Color32::LIGHT_GRAY, Color32::WHITE, Color32::DARK_GRAY));
        for t in &mut self.tabs {
            t.poll(ctx, &theme, self.clipboard, &mut self.clipboard_use);
        }
        let mut i = self.tabs.len();
        while i > 0 {
            i -= 1;
            if self.tabs[i].exited {
                let t = self.tabs.remove(i);
                if let Some(repl) = t.repl.clone().filter(|_| !t.pending_send.is_empty()) {
                    self.repl_failed = Some(repl);
                }
                if self.active > i {
                    self.active -= 1;
                }
            }
        }
        self.active = self.active.min(self.tabs.len().saturating_sub(1));
        if self.tabs.is_empty() {
            self.visible = false;
            return true;
        }
        false
    }

    /// Header (tabs, +, hide) and the active terminal.
    pub fn show(&mut self, ui: &mut egui::Ui, theme: &Theme, font_size: f32) -> Option<PanelAction> {
        self.theme = Some(theme.clone());
        let mut action = None;
        let now = ui.input(|i| i.time);
        self.body = None;
        self.tab_labels.clear();
        let header_bg = ui.painter().add(egui::Shape::Noop);
        let header = ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let mut close = None;
            for (i, t) in self.tabs.iter().enumerate() {
                let ringing = now - t.bell < 0.25;
                let mut text = egui::RichText::new(t.title()).small();
                if ringing {
                    text = text.color(ui.visuals().warn_fg_color);
                }
                let label = ui.selectable_label(i == self.active, text).on_hover_text(t.title());
                if label.clicked() {
                    self.active = i;
                    self.focus_next = true;
                }
                self.tab_labels.push((i, label));
                if ui.small_button("×").on_hover_text("Close this terminal").clicked() {
                    close = Some(i);
                }
                ui.add_space(6.0);
            }
            if ui.small_button("+").on_hover_text(format!("New terminal ({})", Cmd::NewTerminal.shortcut_label().unwrap_or_default())).clicked() {
                action = Some(PanelAction::NewTab);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("×").on_hover_text(format!("Hide the terminal ({})", Cmd::ToggleTerminal.shortcut_label().unwrap_or_default())).clicked() {
                    action = Some(PanelAction::Hide);
                }
            });
            if let Some(i) = close
                && self.close(i)
            {
                action = Some(PanelAction::Hide);
            }
        });
        let full = ui.max_rect();
        let header_rect = egui::Rect::from_min_max(full.min, egui::pos2(full.right(), header.response.rect.bottom() + ui.spacing().item_spacing.y));
        ui.painter().set(header_bg, egui::Shape::rect_filled(header_rect.expand2(egui::vec2(8.0, 4.0)), 0.0, ui.visuals().panel_fill));
        let rect = ui.available_rect_before_wrap();
        let resp = ui.interact(rect, self.id, Sense::click_and_drag());
        ui.allocate_rect(rect, Sense::hover());
        ui.painter().rect_filled(rect.expand2(egui::vec2(8.0, 4.0)), 0.0, theme.bg.gamma_multiply(self.opacity));
        let Some(t) = self.tabs.get_mut(self.active) else {
            return action;
        };
        self.body = Some(resp.clone());
        if self.focus_next || resp.clicked() || resp.drag_started() {
            resp.request_focus();
            self.focus_next = false;
        }
        let focused = resp.has_focus();
        if focused {
            ui.memory_mut(|m| m.set_focus_lock_filter(self.id, egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true }));
        }

        let font = FontId::monospace(font_size);
        let (row_h, char_w) = ui.fonts_mut(|f| (f.row_height(&font), f.glyph_width(&font, 'M')));
        let cell = (char_w.max(1.0), row_h.ceil().max(1.0));
        let pad = 6.0;
        let area = rect.shrink2(vec2(pad, 4.0));
        let cols = (area.width() / cell.0).floor().max(2.0) as usize;
        let rows = (area.height() / cell.1).floor().max(1.0) as usize;
        t.resize(cols, rows, cell);

        if focused != t.focused {
            t.focused = focused;
            let mut term = t.term.lock();
            term.is_focused = focused;
            let report = term.mode().contains(TermMode::FOCUS_IN_OUT);
            drop(term);
            if report {
                t.write(if focused { b"\x1b[I".to_vec() } else { b"\x1b[O".to_vec() });
            }
        }
        if focused {
            keyboard(ui, t, self.option_as_meta);
        } else {
            t.preedit.clear();
        }
        if let Some(open) = links(ui, t, &resp, area.min, cell, &self.roots) {
            action = Some(open);
        }
        if mouse(ui, t, &resp, area.min, cell) {
            // The program took the right-click: no context menu this time.
            self.body = None;
        }
        if t.link.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        let cursor = paint(ui, t, theme, &font, area.min, cell, focused);
        if focused && let Some(cursor_rect) = cursor {
            // Turns on the OS input method and puts its candidate window at the cursor.
            ui.output_mut(|o| o.ime = Some(egui::output::IMEOutput { purpose: egui::IMEPurpose::Terminal, rect: area, cursor_rect, should_interrupt_composition: false }));
        }
        if now - t.bell < 0.25 {
            ui.painter().rect_stroke(rect.shrink(1.0), 0.0, Stroke::new(2.0, theme.ansi[3].gamma_multiply(0.6)), egui::StrokeKind::Inside);
        }
        action
    }
}

/// Turn this frame's keyboard input into PTY bytes, and consume it.
/// `alt_meta` is the `terminal_option_as_meta` setting.
fn keyboard(ui: &egui::Ui, t: &mut Terminal, alt_meta: bool) {
    let (events, mods) = ui.input(|i| (i.events.clone(), i.modifiers));
    let app_cursor = t.term.lock().mode().contains(TermMode::APP_CURSOR);
    // Alt as Meta sends ESC + key (from the Key event), so skip the composed text.
    let meta = alt_meta && mods.alt && !mods.ctrl && !mods.mac_cmd;
    // Windows reports AltGr as Ctrl+Alt: when that comes with text, the text
    // is what was typed (AltGr+Q is @ on a German keyboard), not Ctrl+Alt+Q.
    let altgr = !MAC && mods.ctrl && mods.alt && events.iter().any(|e| matches!(e, Event::Text(s) if !s.is_empty()));
    let mut out = Vec::new();
    for e in &events {
        match e {
            Event::Text(text) if !meta && t.preedit.is_empty() => out.extend_from_slice(text.as_bytes()),
            Event::Ime(ime) => {
                if let Some(text) = t.ime(ime.clone()) {
                    out.extend_from_slice(text.as_bytes());
                }
            }
            // Keys mid-composition belong to the input method.
            Event::Key { .. } if !t.preedit.is_empty() => {}
            Event::Key { key, pressed: true, .. } if altgr && key_bytes(*key, Modifiers::NONE, false, false).is_none() => {}
            Event::Key { key, pressed: true, modifiers, .. } => {
                if MAC && modifiers.mac_cmd && *key == Key::K && !modifiers.shift && !modifiers.alt {
                    t.clear();
                } else if modifiers.shift && !modifiers.ctrl && matches!(key, Key::PageUp | Key::PageDown) {
                    // Shift+PgUp/PgDn page through the scrollback.
                    t.term.lock().scroll_display(if *key == Key::PageUp { Scroll::PageUp } else { Scroll::PageDown });
                } else if let Some(b) = key_bytes(*key, *modifiers, app_cursor, alt_meta) {
                    out.extend(b);
                }
            }
            Event::Copy | Event::Cut => {
                // macOS: Cmd+C. Elsewhere Ctrl+C arrives as Copy too: copy a
                // selection, otherwise it's an interrupt (Ctrl+Shift+C only copies).
                match t.selection_text() {
                    Some(s) => {
                        ui.ctx().copy_text(s);
                        t.term.lock().selection = None;
                    }
                    None if !MAC && !mods.shift => out.push(if matches!(e, Event::Copy) { 0x03 } else { 0x18 }),
                    None => {}
                }
            }
            Event::Paste(text) => {
                t.type_bytes(std::mem::take(&mut out));
                t.paste(text);
            }
            _ => {}
        }
    }
    t.type_bytes(out);
    ui.ctx().input_mut(|i| i.events.retain(|e| !matches!(e, Event::Text(_) | Event::Key { .. } | Event::Copy | Event::Cut | Event::Paste(_) | Event::Ime(_))));
}

/// A mouse event reported to a program.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum MouseAction {
    Press,
    Release,
    /// The pointer moved to another cell (with a button held, or not).
    Motion,
}

/// xterm's button numbers besides left (0), middle (1) and right (2): none
/// (motion with no button held), wheel up, wheel down.
const MOUSE_NONE: u8 = 3;
const WHEEL_UP: u8 = 64;
const WHEEL_DOWN: u8 = 65;

/// The bytes that report a mouse event to a program that turned on mouse
/// reporting, in the encoding it asked for: SGR (`ESC [ < b ; x ; y M/m`),
/// UTF-8, or the classic X10 bytes (which can't address past column 223, so
/// nothing is sent there). `col` and `line` count cells from 0.
pub fn mouse_report(mode: TermMode, button: u8, action: MouseAction, m: Modifiers, col: usize, line: usize) -> Option<Vec<u8>> {
    let mods = 4 * m.shift as u8 + 8 * m.alt as u8 + 16 * m.ctrl as u8;
    let motion = if action == MouseAction::Motion { 32 } else { 0 };
    let (x, y) = (col + 1, line + 1);
    if mode.contains(TermMode::SGR_MOUSE) {
        let end = if action == MouseAction::Release { 'm' } else { 'M' };
        return Some(format!("\x1b[<{};{x};{y}{end}", button + mods + motion).into_bytes());
    }
    // The classic encodings say "a button was released", not which.
    let b = if action == MouseAction::Release { MOUSE_NONE } else { button };
    let values = [32 + (b + mods + motion) as usize, 32 + x, 32 + y];
    let mut out = b"\x1b[M".to_vec();
    for v in values {
        if mode.contains(TermMode::UTF8_MOUSE) {
            let c = char::from_u32(v as u32).filter(|_| v < 0x800)?;
            out.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes());
        } else {
            out.push(u8::try_from(v).ok()?);
        }
    }
    Some(out)
}

/// The modifier that makes file locations clickable: ⌘ on macOS, Ctrl elsewhere.
fn link_modifier(m: Modifiers) -> bool {
    if MAC { m.mac_cmd } else { m.ctrl }
}

/// The cell under `p`, as a grid point (scrollback lines are negative).
fn grid_point(t: &Terminal, p: egui::Pos2, origin: egui::Pos2, cell: (f32, f32), offset: usize) -> Option<Point> {
    let (x, y) = ((p.x - origin.x) / cell.0, (p.y - origin.y) / cell.1);
    if x < 0.0 || y < 0.0 || x >= t.size.cols as f32 || y >= t.size.rows as f32 {
        return None;
    }
    Some(Point::new(Line(y as i32 - offset as i32), Column(x as usize)))
}

/// How many rows above and below the pointer a wrapped line is followed.
const MAX_WRAPPED_ROWS: i32 = 40;

/// The logical line through grid line `line`: the rows the program wrote as
/// one line, joined where the terminal wrapped them, with the grid point of
/// each character (by byte offset). Trailing blanks are left out.
fn logical_line<L: EventListener>(term: &Term<L>, line: Line) -> (String, Vec<(usize, Point)>) {
    let grid = term.grid();
    let (top, bottom, last_col) = (grid.topmost_line(), grid.bottommost_line(), grid.last_column());
    let wraps = |l: Line| grid[l][last_col].flags.contains(Flags::WRAPLINE);
    let (mut first, mut last) = (line, line);
    // A huge wrapped line (minified code, say) is only searched near the
    // pointer, so holding the modifier over it stays cheap.
    while first > top && wraps(first - 1) && line.0 - first.0 < MAX_WRAPPED_ROWS {
        first -= 1;
    }
    while last < bottom && wraps(last) && last.0 - line.0 < MAX_WRAPPED_ROWS {
        last += 1;
    }
    let mut text = String::new();
    let mut at = Vec::new();
    for l in first.0..=last.0 {
        let row = &grid[Line(l)];
        for c in 0..=last_col.0 {
            let cell = &row[Column(c)];
            if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                continue;
            }
            at.push((text.len(), Point::new(Line(l), Column(c))));
            text.push(if cell.c == '\t' { ' ' } else { cell.c });
        }
    }
    text.truncate(text.trim_end().len());
    at.retain(|&(b, _)| b < text.len());
    (text, at)
}

/// The file `file` (as printed) names: absolute, or the first of `bases`
/// holding it. Folders and files that don't exist (by `is_file`) give `None`.
fn resolve_file(file: &str, bases: &[PathBuf], mut is_file: impl FnMut(&Path) -> bool) -> Option<PathBuf> {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from);
    let path = match (file.strip_prefix("~/").or_else(|| file.strip_prefix("~\\")), home) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(file),
    };
    if path.is_absolute() {
        return Some(crate::build::normalize(&path)).filter(|p| is_file(p));
    }
    bases.iter().map(|b| crate::build::normalize(&b.join(&path))).find(|p| is_file(p))
}

/// The clickable location at grid point `p`, if any: found in its logical
/// line and resolved against the shell's folder, where it started, then the
/// side bar folders. Only the location under the pointer is looked up, by
/// the resolver; until it answers there's no link.
fn link_at(ctx: &egui::Context, t: &mut Terminal, p: Point, roots: &[PathBuf]) -> Option<Link> {
    let (text, at, p) = {
        let term = t.term.lock();
        // The right half of a wide character belongs to the character.
        let p = if term.grid()[p].flags.contains(Flags::WIDE_CHAR_SPACER) { Point::new(p.line, Column(p.column.0.saturating_sub(1))) } else { p };
        let (text, at) = logical_line(&term, p.line);
        (text, at, p)
    };
    let byte = at.iter().find(|(_, q)| *q == p)?.0;
    if t.lookup.line.as_ref().is_none_or(|(cached, _)| *cached != text) {
        let mut found = crate::build::find_locations(&text);
        found.truncate(MAX_LOCATIONS_PER_LINE);
        t.lookup.line = Some((text, found));
    }
    let loc = t.lookup.line.as_ref()?.1.iter().find(|l| l.span.contains(&byte))?.clone();
    if t.resolver.is_none() {
        t.resolver = Resolver::start(ctx.clone());
    }
    let resolver = t.resolver.as_ref()?;
    let now = std::time::Instant::now();
    while let Ok((generation, key, path)) = resolver.answers.try_recv() {
        if generation == t.generation {
            t.lookup.answers.insert(key, (now, path));
        }
    }
    if t.roots_seen != roots {
        t.roots_seen = roots.to_vec();
        t.roots_generation += 1;
    }
    let generation = t.generation;
    let key: AnswerKey = (t.raw_shell_dir(), t.roots_generation, loc.file.clone());
    let lookup = &mut t.lookup;
    let fresh = |at: &std::time::Instant| now.duration_since(*at) < FILE_CHECK_TTL;
    match lookup.answers.get(&key) {
        Some((asked_at, answer)) if fresh(asked_at) => {
            let path = answer.clone()?;
            return Some(Link { rows: link_rows(&at, &loc), path, line: loc.line, col: loc.col });
        }
        Some(_) => {
            lookup.answers.remove(&key);
        }
        None => {}
    }
    if lookup.asked.get(&key).is_none_or(|at| !fresh(at)) {
        lookup.asked.insert(key.clone(), now);
        let resolver = t.resolver.as_ref()?;
        let _ = resolver.ask.send(Resolve { generation, key, cwd: t.cwd.clone(), roots: roots.to_vec() });
    }
    None
}

/// The cells `loc` covers, a row at a time.
fn link_rows(at: &[(usize, Point)], loc: &crate::build::TextLocation) -> Vec<LinkRow> {
    let mut rows: Vec<LinkRow> = Vec::new();
    for &(_, q) in at.iter().filter(|(b, _)| loc.span.contains(b)) {
        match rows.last_mut() {
            Some(r) if r.0 == q.line => r.2 = q.column.0 + 1,
            _ => rows.push((q.line, q.column.0, q.column.0 + 1)),
        }
    }
    rows
}

/// ⌘-hover (Ctrl elsewhere) underlines a file location; clicking it opens
/// the file. Not while a program is using the mouse (unless Shift is held,
/// as for selecting), since the click is the program's then.
fn links(ui: &egui::Ui, t: &mut Terminal, resp: &egui::Response, origin: egui::Pos2, cell: (f32, f32), roots: &[PathBuf]) -> Option<PanelAction> {
    let mods = ui.input(|i| i.modifiers);
    let (mode, offset) = {
        let term = t.term.lock();
        (*term.mode(), term.grid().display_offset())
    };
    let reporting = mode.intersects(TermMode::MOUSE_MODE) && !mods.shift;
    let point = resp.hover_pos().filter(|_| link_modifier(mods) && !reporting).and_then(|p| grid_point(t, p, origin, cell, offset));
    let Some(point) = point else {
        t.link = None;
        if t.lookup.line.is_some() || !t.lookup.asked.is_empty() {
            t.lookup = LinkLookup::default();
            t.generation += 1;
        }
        return None;
    };
    t.link = link_at(ui.ctx(), t, point, roots);
    let link = t.link.as_ref()?;
    resp.clicked().then(|| PanelAction::Open(link.path.clone(), link.line, link.col))
}

/// Selection, focus clicks, mouse reporting and the scroll wheel. Returns
/// true when a right-click went to the program (so no context menu opens).
fn mouse(ui: &egui::Ui, t: &mut Terminal, resp: &egui::Response, origin: egui::Pos2, cell: (f32, f32)) -> bool {
    let (cols, rows) = (t.size.cols, t.size.rows);
    let to_point = |p: egui::Pos2, offset: usize| -> (Point, Side) {
        let x = ((p.x - origin.x) / cell.0).max(0.0);
        let col = (x.floor() as usize).min(cols - 1);
        let row = (((p.y - origin.y) / cell.1).floor().max(0.0) as usize).min(rows - 1);
        let side = if x.fract() < 0.5 { Side::Left } else { Side::Right };
        (Point::new(Line(row as i32 - offset as i32), Column(col)), side)
    };
    let to_cell = |p: egui::Pos2| -> (usize, usize) {
        let (p, _) = to_point(p, 0);
        (p.column.0, p.line.0 as usize)
    };
    let (mode, offset) = {
        let term = t.term.lock();
        (*term.mode(), term.grid().display_offset())
    };
    let mods = ui.input(|i| i.modifiers);
    // A program that asks for the mouse gets it; Shift keeps it for selecting.
    let mouse_mode = mode.intersects(TermMode::MOUSE_MODE);
    let reporting = mouse_mode && !mods.shift;
    if resp.hovered() {
        ui.ctx().set_cursor_icon(if reporting { egui::CursorIcon::Default } else { egui::CursorIcon::Text });
    }

    let mut right_reported = false;
    if mouse_mode {
        let ours = |p: egui::Pos2| resp.rect.contains(p) && ui.ctx().layer_id_at(p).is_none_or(|l| l == resp.layer_id);
        for e in ui.input(|i| i.events.clone()) {
            match e {
                Event::PointerButton { pos, button, pressed, modifiers } => {
                    let b = match button {
                        egui::PointerButton::Primary => 0,
                        egui::PointerButton::Middle => 1,
                        egui::PointerButton::Secondary => 2,
                        _ => continue,
                    };
                    let down = &mut t.mouse_down[b as usize];
                    if pressed {
                        if modifiers.shift || !ours(pos) {
                            continue;
                        }
                        *down = true;
                    } else if !std::mem::take(down) {
                        // Pressed elsewhere, or with Shift: not the program's.
                        continue;
                    }
                    right_reported |= b == 2;
                    let (col, line) = to_cell(pos);
                    t.mouse_cell = Some((col, line));
                    let action = if pressed { MouseAction::Press } else { MouseAction::Release };
                    t.write(mouse_report(mode, b, action, modifiers, col, line).unwrap_or_default());
                }
                Event::PointerMoved(pos) => {
                    let held = t.mouse_down.iter().position(|d| *d);
                    let wanted = match held {
                        Some(_) => mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION),
                        None => mode.contains(TermMode::MOUSE_MOTION) && reporting && ours(pos),
                    };
                    let c = to_cell(pos);
                    if !wanted || t.mouse_cell == Some(c) {
                        continue;
                    }
                    t.mouse_cell = Some(c);
                    let button = held.map_or(MOUSE_NONE, |b| b as u8);
                    t.write(mouse_report(mode, button, MouseAction::Motion, mods, c.0, c.1).unwrap_or_default());
                }
                _ => {}
            }
        }
    } else {
        t.mouse_down = [false; 3];
    }

    if let Some(pos) = resp.interact_pointer_pos().filter(|_| !reporting) {
        let kind = if resp.triple_clicked() {
            Some(SelectionType::Lines)
        } else if resp.double_clicked() {
            Some(SelectionType::Semantic)
        } else {
            None
        };
        if let Some(kind) = kind {
            let (p, side) = to_point(pos, offset);
            let mut sel = Selection::new(kind, p, side);
            sel.update(p, side);
            t.term.lock().selection = Some(sel);
        } else if resp.drag_started_by(egui::PointerButton::Primary) {
            // Right-drags and right-clicks leave the selection alone (context menu).
            let start = ui.input(|i| i.pointer.press_origin()).unwrap_or(pos);
            let (p, side) = to_point(start, offset);
            let mut sel = Selection::new(SelectionType::Simple, p, side);
            let (p, side) = to_point(pos, offset);
            sel.update(p, side);
            t.term.lock().selection = Some(sel);
        } else if resp.dragged_by(egui::PointerButton::Primary) {
            let (p, side) = to_point(pos, offset);
            if let Some(sel) = t.term.lock().selection.as_mut() {
                sel.update(p, side);
            }
        } else if resp.clicked() {
            t.term.lock().selection = None;
        }
    }

    if !resp.hovered() {
        return right_reported;
    }
    let dy = ui.input(|i| i.smooth_scroll_delta.y);
    if dy == 0.0 {
        return right_reported;
    }
    t.scroll_acc += dy / cell.1;
    let lines = t.scroll_acc.trunc() as i32;
    if lines == 0 {
        return right_reported;
    }
    t.scroll_acc -= lines as f32;
    let n = lines.unsigned_abs() as usize;
    if reporting {
        let (col, line) = to_cell(ui.input(|i| i.pointer.hover_pos()).unwrap_or(origin));
        let button = if lines > 0 { WHEEL_UP } else { WHEEL_DOWN };
        t.write(mouse_report(mode, button, MouseAction::Press, mods, col, line).unwrap_or_default().repeat(n));
    } else if mode.contains(TermMode::ALT_SCREEN) && mode.contains(TermMode::ALTERNATE_SCROLL) && !mods.shift {
        // Full-screen programs (less, vim…) scroll with arrow keys.
        let key = if lines > 0 { Key::ArrowUp } else { Key::ArrowDown };
        let one = key_bytes(key, Modifiers::NONE, mode.contains(TermMode::APP_CURSOR), false).unwrap_or_default();
        t.write(one.repeat(n));
    } else {
        t.term.lock().scroll_display(Scroll::Delta(lines));
    }
    right_reported
}

/// The visible cells, colored, with the cursor's cell and shape.
fn screen_cells<L: EventListener>(term: &Term<L>, theme: &Theme, cols: usize, rows: usize) -> (Vec<Cell>, Option<(usize, usize)>, CursorShape) {
    let blank = Cell { c: ' ', extra: None, fg: theme.fg, bg: theme.bg, flags: Flags::empty() };
    let mut grid = vec![blank; cols * rows];
    let content = term.renderable_content();
    let offset = content.display_offset;
    let colors = content.colors;
    let selection = content.selection;
    for ic in content.display_iter {
        let Some(vp) = alacritty_terminal::term::point_to_viewport(offset, ic.point) else { continue };
        if vp.line >= rows || vp.column.0 >= cols {
            continue;
        }
        let flags = ic.cell.flags;
        let mut fg = theme.resolve(ic.cell.fg, colors);
        let mut bg = theme.resolve(ic.cell.bg, colors);
        if flags.contains(Flags::DIM) {
            fg = mix(bg, fg, 0.66);
        }
        if flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
        }
        if flags.contains(Flags::HIDDEN) {
            fg = bg;
        }
        if selection.is_some_and(|s| s.contains(ic.point)) {
            bg = theme.selection;
        }
        // The emulator marks where a tab started with a '\t' (so copying keeps
        // it); on screen that cell is blank, like the ones the tab skipped.
        let c = if ic.cell.c == '\t' { ' ' } else { ic.cell.c };
        let extra = ic.cell.zerowidth().map(|z| z.iter().collect::<String>().into_boxed_str());
        grid[vp.line * cols + vp.column.0] = Cell { c, extra, fg, bg, flags };
    }
    let c = content.cursor;
    let cursor = alacritty_terminal::term::point_to_viewport(offset, c.point).filter(|p| p.line < rows && p.column.0 < cols).map(|p| (p.line, p.column.0));
    (grid, cursor, c.shape)
}

/// Emoji skin-tone modifiers. The emulator gives each its own two cells after
/// the emoji, but egui can't combine them with it (no text shaping), and alone
/// they're a box or a color swatch, so those cells are left blank.
fn is_emoji_modifier(c: char) -> bool {
    ('\u{1F3FB}'..='\u{1F3FF}').contains(&c)
}

/// One piece of text drawn from `col` in the style of that cell.
#[derive(Debug, PartialEq)]
struct Run {
    col: usize,
    text: String,
}

/// A row's text, split so every piece starts at its own grid column: ASCII
/// runs of one style share a galley; anything else (wide, combined, fallback
/// glyphs) is placed cell by cell with its combining marks, variation
/// selectors and joiners (which egui draws as nothing when it can't use them).
fn text_runs(line: &[Cell]) -> Vec<Run> {
    let style = |c: &Cell| (c.fg, c.flags & (Flags::BOLD | Flags::ITALIC | Flags::ALL_UNDERLINES | Flags::STRIKEOUT));
    let mut runs = Vec::new();
    let mut col = 0;
    while col < line.len() {
        let c = &line[col];
        if c.flags.contains(Flags::WIDE_CHAR_SPACER) || (c.c == ' ' && c.extra.is_none() && !c.flags.intersects(Flags::ALL_UNDERLINES | Flags::STRIKEOUT)) {
            col += 1;
            continue;
        }
        let start = col;
        let mut text = String::new();
        if c.c.is_ascii() && c.extra.is_none() {
            let s = style(c);
            while col < line.len() && line[col].c.is_ascii() && line[col].extra.is_none() && style(&line[col]) == s {
                text.push(line[col].c);
                col += 1;
            }
        } else {
            if !is_emoji_modifier(c.c) {
                text.push(c.c);
                text.push_str(c.extra.as_deref().unwrap_or_default());
            }
            col += if c.flags.contains(Flags::WIDE_CHAR) { 2 } else { 1 };
        }
        if !text.is_empty() {
            runs.push(Run { col: start, text });
        }
    }
    runs
}

/// Paint the grid: backgrounds, then text in runs of same-styled cells, then
/// the cursor and any text being composed. Returns the cursor's rect.
fn paint(ui: &egui::Ui, t: &Terminal, theme: &Theme, font: &FontId, origin: egui::Pos2, cell: (f32, f32), focused: bool) -> Option<Rect> {
    let (cols, rows) = (t.size.cols, t.size.rows);
    let term = t.term.lock();
    let (mut grid, cursor_cell, cursor_shape) = screen_cells(&term, theme, cols, rows);
    let offset = term.grid().display_offset() as i32;
    drop(term);
    let painter = ui.painter();
    let x_of = |col: usize| origin.x + col as f32 * cell.0;
    let y_of = |row: usize| origin.y + row as f32 * cell.1;
    let composing = !t.preedit.is_empty();

    // The cursor cell is drawn inverted when it's a focused block.
    if let (Some((r, c)), true, CursorShape::Block, false) = (cursor_cell, focused, cursor_shape, composing) {
        let cell = &mut grid[r * cols + c];
        cell.fg = theme.bg;
        cell.bg = theme.cursor;
    }

    for row in 0..rows {
        let line = &grid[row * cols..(row + 1) * cols];
        let y = y_of(row);
        // Backgrounds, merged across runs of the same color.
        let mut col = 0;
        while col < cols {
            let bg = line[col].bg;
            let start = col;
            while col < cols && line[col].bg == bg {
                col += 1;
            }
            if bg != theme.bg {
                painter.rect_filled(Rect::from_min_max(pos2(x_of(start), y), pos2(x_of(col), y + cell.1)), 0.0, bg);
            }
        }
        for Run { col: start, text } in text_runs(line) {
            let c = &line[start];
            if !text.is_ascii() {
                crate::fonts::ensure_cjk(ui.ctx(), &text);
            }
            let mut fmt = egui::TextFormat::simple(font.clone(), c.fg);
            fmt.italics = c.flags.contains(Flags::ITALIC);
            if c.flags.intersects(Flags::ALL_UNDERLINES) {
                fmt.underline = Stroke::new(1.0, c.fg);
            }
            if c.flags.contains(Flags::STRIKEOUT) {
                fmt.strikethrough = Stroke::new(1.0, c.fg);
            }
            let galley = painter.layout_job(egui::text::LayoutJob::single_section(text, fmt));
            let pos = pos2(x_of(start), y);
            if c.flags.contains(Flags::BOLD) {
                // No bold monospace face: overstrike, like old terminals.
                painter.galley(pos + vec2(0.6, 0.0), galley.clone(), c.fg);
            }
            painter.galley(pos, galley, c.fg);
        }
    }
    for &(line, from, to) in t.link.iter().flat_map(|l| &l.rows) {
        let Ok(row) = usize::try_from(line.0 + offset) else { continue };
        if row < rows {
            let fg = grid[row * cols + from.min(cols - 1)].fg;
            let y = y_of(row) + cell.1 - 1.0;
            painter.line_segment([pos2(x_of(from), y), pos2(x_of(to.min(cols)), y)], Stroke::new(1.0, fg));
        }
    }

    let (r, c) = cursor_cell?;
    let at = pos2(x_of(c), y_of(r));
    if composing {
        // Input-method text, underlined over the cursor until it's committed.
        crate::fonts::ensure_cjk(ui.ctx(), &t.preedit);
        let mut fmt = egui::TextFormat::simple(font.clone(), theme.fg);
        fmt.underline = Stroke::new(1.0, theme.fg);
        fmt.background = theme.bg;
        let galley = painter.layout_job(egui::text::LayoutJob::single_section(t.preedit.clone(), fmt));
        let w = galley.size().x;
        painter.rect_filled(Rect::from_min_size(at, vec2(w, cell.1)), 0.0, theme.bg);
        painter.galley(at, galley, theme.fg);
        painter.rect_filled(Rect::from_min_size(at + vec2(w, 0.0), vec2(2.0, cell.1)), 0.0, theme.cursor);
        return Some(Rect::from_min_size(at + vec2(w, 0.0), vec2(1.0, cell.1)));
    }
    let wide = grid[r * cols + c].flags.contains(Flags::WIDE_CHAR);
    let w = if wide { cell.0 * 2.0 } else { cell.0 };
    let rect = Rect::from_min_size(at, vec2(w, cell.1));
    match (focused, cursor_shape) {
        (_, CursorShape::Hidden) => {}
        (true, CursorShape::Block) => {}
        (true, CursorShape::Beam) => {
            painter.rect_filled(Rect::from_min_size(rect.min, vec2(2.0, cell.1)), 0.0, theme.cursor);
        }
        (true, CursorShape::Underline) => {
            painter.rect_filled(Rect::from_min_max(pos2(rect.left(), rect.bottom() - 2.0), rect.max), 0.0, theme.cursor);
        }
        _ => {
            painter.rect_stroke(rect.shrink(0.5), 0.0, Stroke::new(1.0, theme.cursor), egui::StrokeKind::Inside);
        }
    }
    Some(Rect::from_min_size(at, vec2(1.0, cell.1)))
}

/// Where a new terminal starts: the project folder, else the file's folder, else home.
pub fn start_dir(folder: Option<&Path>, file: Option<&Path>) -> PathBuf {
    folder
        .map(Path::to_path_buf)
        .or_else(|| file.and_then(Path::parent).filter(|p| p.is_dir()).map(Path::to_path_buf))
        .or_else(|| std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(key: Key, m: Modifiers) -> Vec<u8> {
        key_bytes(key, m, false, true).unwrap_or_default()
    }

    #[test]
    fn special_keys_send_vt_sequences() {
        assert_eq!(bytes(Key::Enter, Modifiers::NONE), b"\r");
        assert_eq!(bytes(Key::Backspace, Modifiers::NONE), b"\x7f");
        assert_eq!(bytes(Key::Tab, Modifiers::NONE), b"\t");
        assert_eq!(bytes(Key::Tab, Modifiers::SHIFT), b"\x1b[Z");
        assert_eq!(bytes(Key::Escape, Modifiers::NONE), b"\x1b");
        assert_eq!(bytes(Key::ArrowUp, Modifiers::NONE), b"\x1b[A");
        assert_eq!(key_bytes(Key::ArrowUp, Modifiers::NONE, true, true).unwrap(), b"\x1bOA", "DECCKM");
        assert_eq!(bytes(Key::ArrowLeft, Modifiers::CTRL), b"\x1b[1;5D");
        assert_eq!(bytes(Key::ArrowRight, Modifiers::SHIFT), b"\x1b[1;2C");
        assert_eq!(bytes(Key::Home, Modifiers::NONE), b"\x1b[H");
        assert_eq!(bytes(Key::Delete, Modifiers::NONE), b"\x1b[3~");
        assert_eq!(bytes(Key::PageDown, Modifiers::NONE), b"\x1b[6~");
        assert_eq!(bytes(Key::F1, Modifiers::NONE), b"\x1bOP");
        assert_eq!(bytes(Key::F5, Modifiers::NONE), b"\x1b[15~");
        assert_eq!(bytes(Key::F12, Modifiers::SHIFT), b"\x1b[24;2~");
    }

    #[test]
    fn ctrl_and_alt_keys() {
        assert_eq!(bytes(Key::C, Modifiers::CTRL), [0x03]);
        assert_eq!(bytes(Key::A, Modifiers::CTRL), [0x01]);
        assert_eq!(bytes(Key::Z, Modifiers::CTRL), [0x1a]);
        assert_eq!(bytes(Key::Space, Modifiers::CTRL), [0x00]);
        assert_eq!(bytes(Key::OpenBracket, Modifiers::CTRL), [0x1b]);
        assert_eq!(bytes(Key::B, Modifiers::ALT), b"\x1bb");
        assert_eq!(bytes(Key::B, Modifiers::ALT | Modifiers::SHIFT), b"\x1bB");
        assert_eq!(bytes(Key::Num1, Modifiers::ALT | Modifiers::SHIFT), b"\x1b!");
        assert_eq!(bytes(Key::Period, Modifiers::ALT), b"\x1b.");
        assert_eq!(bytes(Key::Backspace, Modifiers::ALT), b"\x1b\x7f");
        assert_eq!(bytes(Key::X, Modifiers::CTRL | Modifiers::ALT), b"\x1b\x18");
        // Plain letters type through Text events, not key bytes.
        assert_eq!(key_bytes(Key::A, Modifiers::NONE, false, true), None);
        if MAC {
            assert_eq!(key_bytes(Key::C, Modifiers::MAC_CMD | Modifiers::COMMAND, false, true), None, "Cmd keys belong to the app");
        }
    }

    #[test]
    fn option_types_text_unless_it_is_meta() {
        let alt = Modifiers::ALT;
        // As Meta: Esc + the key.
        assert_eq!(key_bytes(Key::L, alt, false, true).unwrap(), b"\x1bl");
        // Not Meta: the layout's character comes as text (Option+L is @ on a German Mac).
        assert_eq!(key_bytes(Key::L, alt, false, false), None);
        assert_eq!(key_bytes(Key::Num8, alt | Modifiers::SHIFT, false, false), None);
        // Keys that type nothing keep their Alt meaning either way.
        assert_eq!(key_bytes(Key::Backspace, alt, false, false).unwrap(), b"\x1b\x7f");
        assert_eq!(key_bytes(Key::Enter, alt, false, false).unwrap(), b"\x1b\r");
        assert_eq!(key_bytes(Key::ArrowLeft, alt, false, false).unwrap(), b"\x1b[1;3D");
        assert_eq!(key_bytes(Key::X, Modifiers::CTRL | alt, false, false).unwrap(), b"\x1b\x18");
    }

    #[test]
    fn mouse_reports_in_the_requested_encoding() {
        use MouseAction::*;
        let none = Modifiers::NONE;
        let click = TermMode::MOUSE_REPORT_CLICK;
        let sgr = click | TermMode::SGR_MOUSE;
        // SGR: 1-based cell, M on press, m on release, real button on release.
        assert_eq!(mouse_report(sgr, 0, Press, none, 4, 1).unwrap(), b"\x1b[<0;5;2M");
        assert_eq!(mouse_report(sgr, 0, Release, none, 4, 1).unwrap(), b"\x1b[<0;5;2m");
        assert_eq!(mouse_report(sgr, 2, Press, none, 0, 0).unwrap(), b"\x1b[<2;1;1M");
        assert_eq!(mouse_report(sgr, 0, Motion, none, 9, 2).unwrap(), b"\x1b[<32;10;3M", "drag");
        assert_eq!(mouse_report(sgr, MOUSE_NONE, Motion, none, 9, 2).unwrap(), b"\x1b[<35;10;3M", "hover");
        assert_eq!(mouse_report(sgr, WHEEL_UP, Press, none, 0, 0).unwrap(), b"\x1b[<64;1;1M");
        assert_eq!(mouse_report(sgr, WHEEL_DOWN, Press, Modifiers::CTRL, 0, 0).unwrap(), b"\x1b[<81;1;1M");
        assert_eq!(mouse_report(sgr, 0, Press, Modifiers::ALT | Modifiers::SHIFT, 0, 0).unwrap(), b"\x1b[<12;1;1M");
        assert_eq!(mouse_report(sgr, 1, Press, none, 300, 0).unwrap(), b"\x1b[<1;301;1M", "SGR has no column limit");
        // Classic: three bytes offset by 32, and release is button 3.
        assert_eq!(mouse_report(click, 0, Press, none, 4, 1).unwrap(), b"\x1b[M\x20\x25\x22");
        assert_eq!(mouse_report(click, 2, Release, none, 4, 1).unwrap(), b"\x1b[M\x23\x25\x22");
        assert_eq!(mouse_report(click, WHEEL_UP, Press, none, 0, 0).unwrap(), b"\x1b[M\x60\x21\x21");
        assert_eq!(mouse_report(click, 0, Press, none, 222, 0).unwrap(), b"\x1b[M\x20\xff\x21");
        assert_eq!(mouse_report(click, 0, Press, none, 223, 0), None, "past column 223");
        // UTF-8 extends the range by encoding big values as two bytes.
        let utf8 = click | TermMode::UTF8_MOUSE;
        assert_eq!(mouse_report(utf8, 0, Press, none, 299, 0).unwrap(), "\x1b[M\u{20}\u{14c}\u{21}".as_bytes());
    }

    /// Feed `bytes` to a bare emulator and build the cells `paint` draws.
    fn cells_for(bytes: &[u8], cols: usize, rows: usize) -> Vec<Cell> {
        use alacritty_terminal::event::VoidListener;
        let mut term = Term::new(Config::default(), &Size { cols, rows }, VoidListener);
        let mut parser: alacritty_terminal::vte::ansi::Processor = alacritty_terminal::vte::ansi::Processor::new();
        parser.advance(&mut term, bytes);
        let theme = Theme::new(Color32::BLACK, Color32::WHITE, Color32::WHITE, Color32::GRAY);
        screen_cells(&term, &theme, cols, rows).0
    }

    /// Each visible character and the column it's drawn at. ASCII runs are one
    /// galley (one cell per char); other runs are one cell's text.
    fn placed(runs: &[Run]) -> Vec<(usize, String)> {
        let mut out = Vec::new();
        for r in runs {
            if r.text.is_ascii() {
                out.extend(r.text.chars().enumerate().filter(|(_, c)| *c != ' ').map(|(i, c)| (r.col + i, c.to_string())));
            } else {
                out.push((r.col, r.text.clone()));
            }
        }
        out
    }

    fn at(col: usize, s: &str) -> (usize, String) {
        (col, s.to_owned())
    }

    #[test]
    fn tabs_and_wide_chars_stay_on_the_grid() {
        let cols = 30;
        let grid = cells_for("A\tB 中文|\r\nname\tsize\tx\r\n".as_bytes(), cols, 3);
        let runs = text_runs(&grid[..cols]);
        assert!(runs.iter().all(|r| !r.text.contains('\t')), "no tab reaches egui's layout: {runs:?}");
        // B after the tab stop at column 8, 中 at 10, 文 at 12, | at 14.
        assert_eq!(placed(&runs), [at(0, "A"), at(8, "B"), at(10, "中"), at(12, "文"), at(14, "|")]);
        let row = placed(&text_runs(&grid[cols..2 * cols]));
        assert!(row.contains(&at(0, "n")) && row.contains(&at(8, "s")) && row.contains(&at(16, "x")), "{row:?}");
    }

    #[test]
    fn combining_marks_and_emoji_sequences() {
        let cols = 20;
        // e + combining acute; heart + VS16; thumbs up + skin tone.
        let grid = cells_for("e\u{301}x \u{2764}\u{fe0f} \u{1f44d}\u{1f3fd}|".as_bytes(), cols, 1);
        let all = placed(&text_runs(&grid));
        // The accent and the selector stay with their characters; the skin
        // tone has its own two cells (7–8) from the emulator, drawn blank, and
        // the bar lands after them.
        assert_eq!(all, [at(0, "e\u{301}"), at(1, "x"), at(3, "\u{2764}\u{fe0f}"), at(5, "\u{1f44d}"), at(9, "|")]);
        assert_eq!(grid[7].c, '\u{1f3fd}');
        // A joiner sequence: each emoji in its own cells, the joiner kept with the one before.
        let grid = cells_for("\u{1f468}\u{200d}\u{1f469}|".as_bytes(), cols, 1);
        assert_eq!(placed(&text_runs(&grid)), [at(0, "\u{1f468}\u{200d}"), at(2, "\u{1f469}"), at(4, "|")]);
    }

    #[test]
    fn monospace_runs_advance_exactly_one_cell_per_char() {
        // ASCII runs are laid out as one galley: each glyph must land on its cell.
        let ctx = egui::Context::default();
        ctx.run_ui(egui::RawInput::default(), |_| {}).textures_delta.clear();
        let font = FontId::monospace(14.0);
        let (w, galley) = ctx.fonts_mut(|f| (f.glyph_width(&font, 'M'), f.layout_no_wrap("|1234567 ab".repeat(10), font.clone(), Color32::WHITE)));
        assert!((galley.size().x - 110.0 * w).abs() < 0.5, "{} vs {}", galley.size().x, 110.0 * w);
    }

    #[cfg(unix)]
    #[test]
    fn python_process_names() {
        for name in ["python", "python3", "python3.14", "python3.12.1", "pypy", "pypy3", "pypy3.10"] {
            assert!(is_python(name), "{name}");
        }
        for name in ["sudo", "ssh", "zsh", "node", "python-agent", "pypy-helper", "python3-config", "pythonw.", "python3.", "py", "mypython"] {
            assert!(!is_python(name), "{name}");
        }
        // The macOS framework build runs as "Python"; names there ignore case.
        assert_eq!(is_python("Python"), MAC);
        assert_eq!(is_python("PyPy3.10"), MAC);
    }

    #[test]
    fn indented_last_lines_leave_a_block_open() {
        assert!(open_block("for i in range(2):\n    print(i)"));
        assert!(open_block("if x:\n\tpass\n\n  \n"), "blank lines after it don't count");
        assert!(!open_block("for i in 1 2; do\n  echo $i\ndone"));
        assert!(!open_block("echo hi"));
        assert!(!open_block(""));
    }

    #[test]
    fn paste_is_bracketed_on_request() {
        assert_eq!(paste_bytes("a\nb", false), b"a\rb");
        assert_eq!(paste_bytes("a\r\nb", false), b"a\rb");
        assert_eq!(paste_bytes("x\x1b[201~y", true), b"\x1b[200~x[201~y\x1b[201~");
    }

    #[test]
    fn pasted_text_cannot_escape_the_bracket() {
        // Stripping only the end marker once would rebuild it from a nested one.
        for evil in ["\x1b[20\x1b[201~1~rm -rf ~\n", "a\x1b\x1b[201~[201~b", "x\u{9b}201~y", "\x1b]52;c;?\x07"] {
            let out = paste_bytes(evil, true);
            let body = &out[6..out.len() - 6];
            assert!(out.starts_with(b"\x1b[200~") && out.ends_with(b"\x1b[201~"));
            assert!(!body.contains(&0x1b), "{evil:?} left an ESC inside the bracket");
            assert!(!String::from_utf8_lossy(body).contains('\u{9b}'), "{evil:?} left a C1 CSI");
        }
        // Unbracketed pastes lose their escapes too.
        assert_eq!(paste_bytes("a\x1b[2Jb", false), b"a[2Jb");
        // Ordinary text, including non-ASCII, is untouched.
        assert_eq!(paste_bytes("héllo — ✓", true), "\x1b[200~héllo — ✓\x1b[201~".as_bytes());
    }

    #[test]
    fn colors_follow_the_theme() {
        let dark = Theme::new(hex(0x1e1f22), hex(0xd4d4d4), Color32::WHITE, hex(0x264f78));
        assert_eq!(dark.resolve(AnsiColor::Named(NamedColor::Background), &Colors::default()), hex(0x1e1f22));
        assert_eq!(dark.resolve(AnsiColor::Named(NamedColor::Foreground), &Colors::default()), hex(0xd4d4d4));
        assert_eq!(dark.resolve(AnsiColor::Named(NamedColor::Red), &Colors::default()), hex(0xe06c75));
        assert_eq!(dark.resolve(AnsiColor::Indexed(1), &Colors::default()), hex(0xe06c75));
        // Cube corners and the gray ramp.
        assert_eq!(dark.indexed(16), Color32::from_rgb(0, 0, 0));
        assert_eq!(dark.indexed(196), Color32::from_rgb(255, 0, 0));
        assert_eq!(dark.indexed(231), Color32::from_rgb(255, 255, 255));
        assert_eq!(dark.indexed(232), Color32::from_rgb(8, 8, 8));
        assert_eq!(dark.indexed(255), Color32::from_rgb(238, 238, 238));
        assert_eq!(dark.resolve(AnsiColor::Spec(Rgb { r: 1, g: 2, b: 3 }), &Colors::default()), Color32::from_rgb(1, 2, 3));
        // Programs can repaint palette entries.
        let mut over = Colors::default();
        over[1] = Some(Rgb { r: 9, g: 9, b: 9 });
        assert_eq!(dark.resolve(AnsiColor::Named(NamedColor::Red), &over), Color32::from_rgb(9, 9, 9));

        // Light themes get darker hues and dark "black" text.
        let light = Theme::new(hex(0xfafafa), hex(0x383a42), Color32::BLACK, hex(0xe5e5e6));
        let lum = |c: Color32| c.r() as u32 + c.g() as u32 + c.b() as u32;
        assert!(lum(light.ansi[0]) < lum(dark.ansi[0]) + 200);
        assert!(lum(light.ansi[2]) < lum(dark.ansi[2]), "green is darker on light backgrounds");
        assert!(lum(light.ansi[0]) < 3 * 100, "black stays dark");
        assert!(lum(dark.ansi[15]) > lum(dark.ansi[7]), "bright white is brighter");
    }

    #[test]
    fn terminal_owns_ctrl_keys_but_not_window_shortcuts() {
        assert!(app_keeps(Some(Cmd::ToggleTerminal), Modifiers::CTRL));
        assert!(!app_keeps(Some(Cmd::GotoLine), Modifiers::CTRL));
        if MAC {
            assert!(app_keeps(Some(Cmd::CommandPalette), Modifiers::COMMAND | Modifiers::SHIFT));
        } else {
            assert!(app_keeps(Some(Cmd::CommandPalette), Modifiers::CTRL | Modifiers::SHIFT));
            assert!(!app_keeps(Some(Cmd::Save), Modifiers::CTRL));
        }
    }

    #[test]
    fn start_dir_prefers_the_project_folder() {
        let tmp = std::env::temp_dir();
        assert_eq!(start_dir(Some(Path::new("/proj")), Some(&tmp.join("x.rs"))), PathBuf::from("/proj"));
        assert_eq!(start_dir(None, Some(&tmp.join("x.rs"))), tmp);
    }

    #[test]
    fn curated_schemes_bring_their_terminal_colors() {
        let mut hl = crate::highlight::Highlighter::new();
        hl.set_theme("Dracula");
        let t = Theme::from_highlighter(&hl);
        assert_eq!(t.ansi[1], Color32::from_rgb(0xff, 0x55, 0x55), "Dracula red");
        assert_eq!(t.ansi[6], Color32::from_rgb(0x8b, 0xe9, 0xfd), "Dracula cyan");
        assert_ne!(t.ansi[9], t.ansi[1], "bright red differs");
        hl.set_theme("Strata Dark");
        assert_eq!(Theme::from_highlighter(&hl).ansi[1], Color32::from_rgb(0xe0, 0x6c, 0x75), "fallback hues");
    }

    #[cfg(unix)]
    #[test]
    fn shell_output_reaches_the_grid() {
        let dir = std::env::temp_dir();
        // The quotes keep the command line itself from matching.
        let shell = ("/bin/sh".to_owned(), vec!["-c".to_owned(), "echo hel''lo; echo $TERM; sleep 5".to_owned()]);
        let t = Terminal::spawn(None, &dir, Some(shell), 40, 5).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut screen = String::new();
        while std::time::Instant::now() < deadline {
            screen = t.screen_text();
            if screen.contains("xterm-256color") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(screen.lines().next() == Some("hello"), "{screen:?}");
        assert!(screen.contains("xterm-256color"), "{screen:?}");
        // Dropping hangs up the still-running shell without blocking.
        let pid = t.pid;
        let start = std::time::Instant::now();
        drop(t);
        assert!(start.elapsed() < std::time::Duration::from_millis(500));
        // Running, not merely a zombie waiting for the reaper thread (which is fine).
        let alive = |pid: u32| {
            std::process::Command::new("ps").args(["-o", "stat=", "-p", &pid.to_string()]).output().is_ok_and(|o| {
                let stat = String::from_utf8_lossy(&o.stdout);
                // Z: zombie; E: exiting (macOS), waiting only to be reaped.
                !stat.trim().is_empty() && !stat.contains(['Z', 'E'])
            })
        };
        // Generous: SIGKILL follows after 2 s, and a loaded machine can be slow to reap.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        while alive(pid) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(!alive(pid), "shell {pid} survived its terminal");
    }

    #[cfg(unix)]
    #[test]
    fn programs_cannot_read_the_clipboard_unless_allowed() {
        // OSC 52 with "?" asks the terminal to send back the clipboard.
        let shell = ("/bin/sh".to_owned(), vec!["-c".to_owned(), "printf '\\033]52;c;?\\007'; sleep 5".to_owned()]);
        let ctx = egui::Context::default();
        let mut panel = Panel::new();
        assert!(!panel.clipboard.read, "denied by default");
        panel.tabs.push(Terminal::spawn(None, &std::env::temp_dir(), Some(shell), 20, 3).unwrap());
        panel.visible = true;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !panel.clipboard_use.read_blocked && std::time::Instant::now() < deadline {
            panel.poll(&ctx);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(panel.clipboard_use.read_blocked, "the read was refused and reported");
    }

    /// A shell prints an OSC 52 write; returns what the panel reported and what reached the clipboard.
    #[cfg(unix)]
    fn osc52_write(allow_write: bool) -> (ClipboardUse, Vec<String>) {
        // "aGk=" is "hi" in base64.
        let shell = ("/bin/sh".to_owned(), vec!["-c".to_owned(), "printf '\\033]52;c;aGk=\\007'; sleep 5".to_owned()]);
        let ctx = egui::Context::default();
        let mut panel = Panel::new();
        panel.clipboard.write = allow_write;
        panel.tabs.push(Terminal::spawn(None, &std::env::temp_dir(), Some(shell), 20, 3).unwrap());
        panel.visible = true;
        let mut copied = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while panel.clipboard_use == ClipboardUse::default() && std::time::Instant::now() < deadline {
            let mut out = ctx.run_ui(Default::default(), |_| {
                panel.poll(&ctx);
            });
            out.textures_delta.clear();
            copied.extend(out.platform_output.commands.iter().filter_map(|c| match c {
                egui::OutputCommand::CopyText(t) => Some(t.clone()),
                _ => None,
            }));
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        (panel.clipboard_use, copied)
    }

    #[cfg(unix)]
    #[test]
    fn programs_can_set_the_clipboard_and_it_is_reported() {
        assert!(Panel::new().clipboard.write, "allowed by default, like other terminals");
        let (used, copied) = osc52_write(true);
        assert!(used.wrote && !used.write_blocked, "{used:?}");
        assert_eq!(copied, ["hi"]);
        let (used, copied) = osc52_write(false);
        assert!(used.write_blocked && !used.wrote, "{used:?}");
        assert!(copied.is_empty(), "nothing reached the clipboard");
    }

    #[cfg(unix)]
    #[test]
    fn exited_shell_is_noticed() {
        let shell = ("/bin/sh".to_owned(), vec!["-c".to_owned(), "exit 0".to_owned()]);
        let ctx = egui::Context::default();
        let mut panel = Panel::new();
        panel.tabs.push(Terminal::spawn(None, &std::env::temp_dir(), Some(shell), 20, 3).unwrap());
        panel.visible = true;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut closed = false;
        while !closed && std::time::Instant::now() < deadline {
            closed = panel.poll(&ctx);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(closed && panel.tabs.is_empty() && !panel.visible);
    }

    /// An emulator `cols` wide that has been fed `bytes`.
    fn term_with(bytes: &[u8], cols: usize, rows: usize) -> Term<alacritty_terminal::event::VoidListener> {
        let mut term = Term::new(Config::default(), &Size { cols, rows }, alacritty_terminal::event::VoidListener);
        let mut parser: alacritty_terminal::vte::ansi::Processor = alacritty_terminal::vte::ansi::Processor::new();
        parser.advance(&mut term, bytes);
        term
    }

    #[test]
    fn wrapped_lines_are_joined_for_locations() {
        // 20 columns: the path wraps after "src/very/long/pa" and again.
        let term = term_with(b"$ cargo test\r\nerror at src/very/long/path/name.rs:42:7 here\r\nnext line\r\n", 20, 6);
        for row in 1..=3 {
            let (text, at) = logical_line(&term, Line(row));
            assert_eq!(text, "error at src/very/long/path/name.rs:42:7 here", "from row {row}");
            assert_eq!(at.len(), text.chars().count());
            assert_eq!(at[20].1, Point::new(Line(2), Column(0)), "the 21st character starts the second row");
        }
        assert_eq!(logical_line(&term, Line(0)).0, "$ cargo test");
        assert_eq!(logical_line(&term, Line(4)).0, "next line");
        let locs = crate::build::find_locations(&logical_line(&term, Line(2)).0);
        assert_eq!(locs.len(), 1);
        assert_eq!((locs[0].file.as_str(), locs[0].line, locs[0].col), ("src/very/long/path/name.rs", 41, 6));
        // A line that merely fills the row exactly isn't joined to the next one.
        let term = term_with(b"12345678901234567890\r\nsrc/a.rs:1\r\n", 20, 4);
        assert_eq!(logical_line(&term, Line(0)).0, "12345678901234567890");
        assert_eq!(logical_line(&term, Line(1)).0, "src/a.rs:1");
    }

    #[test]
    fn wide_characters_map_to_their_cells() {
        let term = term_with("中文 x.rs:3".as_bytes(), 20, 2);
        let (text, at) = logical_line(&term, Line(0));
        assert_eq!(text, "中文 x.rs:3");
        // 中 and 文 take two cells each, so x is in column 5.
        let x = text.find('x').unwrap();
        assert_eq!(at.iter().find(|(b, _)| *b == x).unwrap().1.column, Column(5));
    }

    #[test]
    fn only_files_that_exist_resolve() {
        let dir = std::env::temp_dir().join(format!("strata-term-links-{}", std::process::id()));
        let project = dir.join("project");
        std::fs::create_dir_all(project.join("src")).unwrap();
        std::fs::write(project.join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(dir.join("top.py"), "x = 1\n").unwrap();
        let bases = [dir.clone(), project.clone()];
        assert_eq!(resolve_file("top.py", &bases, Path::is_file), Some(dir.join("top.py")), "the shell's folder first");
        assert_eq!(resolve_file("src/main.rs", &bases, Path::is_file), Some(project.join("src/main.rs")), "then the project folders");
        assert_eq!(resolve_file("./src/../src/main.rs", &bases, Path::is_file), Some(project.join("src/main.rs")));
        let abs = project.join("src/main.rs");
        assert_eq!(resolve_file(abs.to_str().unwrap(), &[], Path::is_file), Some(abs.clone()));
        assert_eq!(resolve_file("src/missing.rs", &bases, Path::is_file), None, "files that don't exist aren't links");
        assert_eq!(resolve_file("src", &bases, Path::is_file), None, "nor are folders");
        assert_eq!(resolve_file("12", &bases, Path::is_file), None, "nor a time like 12:30:45");
        // File checks are remembered for a while, then made again.
        let mut lookup = DiskLookup::default();
        let top = dir.join("top.py");
        assert!(lookup.is_file(&top));
        std::fs::remove_file(&top).unwrap();
        assert!(lookup.is_file(&top), "remembered");
        lookup.files.get_mut(&top).unwrap().0 -= FILE_CHECK_TTL;
        assert!(!lookup.is_file(&top), "checked again once stale");
        let mut checks = 0;
        let found = resolve_file("src/main.rs", &bases, |p| {
            checks += 1;
            lookup.is_file(p)
        });
        assert_eq!((found, checks), (Some(project.join("src/main.rs")), 2), "stops at the first folder holding it");
        std::fs::remove_dir_all(dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn the_shells_folder_follows_cd() {
        let dir = std::env::temp_dir().join(format!("strata-term-cd-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        // `exec` keeps the pid: the shell itself changes folder.
        let shell = ("/bin/sh".to_owned(), vec!["-c".to_owned(), "cd sub && echo MOVED && exec sleep 10".to_owned()]);
        let mut t = Terminal::spawn(None, &dir, Some(shell), 40, 5).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !t.screen_text().contains("MOVED") && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let canon = |p: PathBuf| p.canonicalize().unwrap();
        let raw = t.raw_shell_dir();
        assert_eq!(raw.clone().map(canon), Some(canon(dir.join("sub"))));
        let mut disk = DiskLookup::default();
        assert_eq!(disk.spell_dir(raw.as_deref(), &dir, &[]), dir.join("sub"), "spelled as the folder it started in");
        assert_eq!(disk.spell_dir(None, &dir, &[]), dir, "unknown: where it started");
        // Asked again within SHELL_DIR_TTL, the remembered answer is used.
        let elsewhere = Some(PathBuf::from("/elsewhere"));
        t.shell_dir = Some((std::time::Instant::now(), elsewhere.clone()));
        assert_eq!(t.raw_shell_dir(), elsewhere);
        t.shell_dir = Some((std::time::Instant::now() - SHELL_DIR_TTL, elsewhere));
        assert_eq!(t.raw_shell_dir(), raw, "and after it the kernel again");
        drop(t);
        std::fs::remove_dir_all(dir).ok();
    }

    /// A panel running `program` with `args` in the temp folder.
    #[cfg(unix)]
    fn panel_running(ctx: &egui::Context, program: &str, args: &[&str]) -> Panel {
        let mut panel = Panel::new();
        let shell = (program.to_owned(), args.iter().map(|a| a.to_string()).collect());
        panel.tabs.push(Terminal::spawn(Some(ctx.clone()), &std::env::temp_dir(), Some(shell), 80, 10).unwrap());
        panel.visible = true;
        panel
    }

    /// Poll for `secs`, or until the screen contains `until`. Returns the screen.
    #[cfg(unix)]
    fn poll_for(panel: &mut Panel, ctx: &egui::Context, secs: f32, until: Option<&str>) -> String {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs_f32(secs);
        loop {
            panel.poll(ctx);
            let screen = panel.tabs[0].screen_text();
            if until.is_some_and(|u| screen.contains(u)) || std::time::Instant::now() > deadline {
                return screen;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    #[cfg(unix)]
    #[test]
    fn no_extra_enter_for_programs_that_are_not_python() {
        // A password-style prompt after an indented line: an extra Enter would
        // answer it with an empty line. Even a "... " prompt doesn't count
        // unless Python is the program running.
        let ctx = egui::Context::default();
        let script = "stty -echo; echo READY; IFS= read -r a; echo \"got[$a]\"; printf '... '; read -r b; echo \"second[$b]\"; sleep 30";
        let mut panel = panel_running(&ctx, "/bin/sh", &["-c", script]);
        poll_for(&mut panel, &ctx, 10.0, Some("READY"));
        panel.tabs[0].send("  sudo -k; read -s x");
        poll_for(&mut panel, &ctx, 10.0, Some("got["));
        let screen = poll_for(&mut panel, &ctx, 1.0, Some("second["));
        assert!(screen.contains("got[  sudo -k; read -s x]"), "{screen}");
        assert!(!screen.contains("second["), "no Enter was pressed for the next prompt: {screen}");
    }

    #[cfg(unix)]
    #[test]
    fn sends_wait_for_a_slow_shell() {
        // Startup files that take a while (conda, nvm) before the prompt.
        let ctx = egui::Context::default();
        let mut panel = panel_running(&ctx, "/bin/sh", &["-c", "sleep 3; printf 'ready$ '; exec cat -v"]);
        panel.tabs[0].send("echo hi");
        poll_for(&mut panel, &ctx, 2.5, None);
        assert!(panel.tabs[0].has_pending_send(), "still waiting while the shell starts");
        let screen = poll_for(&mut panel, &ctx, 10.0, Some("ready$ echo hi"));
        assert!(screen.contains("ready$ echo hi"), "sent once the prompt is up: {screen}");
        assert!(!panel.tabs[0].has_pending_send());

        // zsh, whose startup file sleeps before the prompt: sent when its line
        // editor turns on bracketed paste, and run.
        if !Path::new("/bin/zsh").exists() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("strata-slow-zsh-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".zshrc"), "sleep 3\nPS1='zready%# '\n").unwrap();
        let script = format!("ZDOTDIR={} exec /bin/zsh -i", dir.display());
        let mut panel = panel_running(&ctx, "/bin/sh", &["-c", &script]);
        panel.tabs[0].send("echo $((5*5))");
        poll_for(&mut panel, &ctx, 2.5, None);
        assert!(panel.tabs[0].has_pending_send(), "still waiting while zsh starts");
        let screen = poll_for(&mut panel, &ctx, 10.0, Some("\n25\n"));
        assert!(screen.lines().any(|l| l.trim() == "25") && screen.contains("zready"), "{screen}");
        std::fs::remove_dir_all(dir).ok();
    }
}
