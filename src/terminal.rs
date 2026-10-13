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
use alacritty_terminal::term::{Config, Term, TermMode};
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

    pub fn from_highlighter(hl: &crate::highlight::Highlighter) -> Self {
        Self::new(hl.background(), hl.foreground(), hl.caret(), hl.selection())
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
pub fn key_bytes(key: Key, m: Modifiers, app_cursor: bool) -> Option<Vec<u8>> {
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
        _ if alt => meta(key_char(key, shift)?.to_string().as_bytes()),
        _ => return None,
    })
}

/// Text to send for a paste: bracketed when the program asked for it.
pub fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    if bracketed {
        // A pasted end marker would let the text escape the bracket.
        let clean = text.replace("\x1b[201~", "");
        format!("\x1b[200~{clean}\x1b[201~").into_bytes()
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
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

/// The window shortcuts pressed this frame that the terminal doesn't own.
pub fn pressed(ctx: &egui::Context) -> Vec<Cmd> {
    let mut out = Vec::new();
    for &cmd in Cmd::ALL {
        for &(mods, key) in cmd.keys() {
            if app_keeps(Some(cmd), mods) && crate::keys::take(ctx, mods, key) {
                out.push(cmd);
            }
        }
    }
    out
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
}

impl EventListener for Listener {
    fn send_event(&self, event: TermEvent) {
        match event {
            TermEvent::Wakeup | TermEvent::MouseCursorDirty | TermEvent::CursorBlinkingChange => {}
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
        let queue = Arc::new(Mutex::new(Vec::new()));
        let listener = Listener { ctx, queue: queue.clone() };
        let term = Arc::new(FairMutex::new(Term::new(Config::default(), &size, listener.clone())));
        let event_loop = EventLoop::new(term.clone(), listener, pty, true, false)?;
        let sender = event_loop.channel();
        let thread = Some(event_loop.spawn());
        let shell_name = Path::new(&program).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or(program);
        Ok(Self { term, sender, queue, thread,
            #[cfg(unix)]
            pid,
            title: None, shell_name, exited: false, size, cell: (8.0, 16.0), scroll_acc: 0.0, bell: f64::NEG_INFINITY, focused: false })
    }

    pub fn title(&self) -> &str {
        self.title.as_deref().filter(|t| !t.trim().is_empty()).unwrap_or(&self.shell_name)
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

    /// Handle what the emulator reported since the last frame.
    fn poll(&mut self, ctx: &egui::Context, theme: &Theme) {
        let events = std::mem::take(&mut *self.queue.lock().unwrap_or_else(|e| e.into_inner()));
        for e in events {
            match e {
                TermEvent::Title(t) => self.title = Some(t),
                TermEvent::ResetTitle => self.title = None,
                TermEvent::ClipboardStore(_, text) => ctx.copy_text(text),
                TermEvent::ClipboardLoad(_, format) => {
                    if let Ok(text) = arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
                        self.write(format(&text));
                    }
                }
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
    }

    /// The visible screen as text, one line per row.
    #[cfg(all(test, unix))]
    pub fn screen_text(&self) -> String {
        let t = self.term.lock();
        let grid = t.grid();
        let mut out = String::new();
        for line in 0..grid.screen_lines() {
            let row: String = (0..grid.columns()).map(|c| grid[Point::new(Line(line as i32), Column(c))].c).collect();
            out.push_str(row.trim_end());
            out.push('\n');
        }
        out
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

#[cfg(unix)]
fn hang_up(pid: u32) {
    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    const SIGHUP: i32 = 1;
    // The whole process group: the shell and the jobs it started.
    unsafe {
        kill(-(pid as i32), SIGHUP);
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
}

pub struct Panel {
    pub tabs: Vec<Terminal>,
    pub active: usize,
    pub visible: bool,
    focus_next: bool,
    id: egui::Id,
    theme: Option<Theme>,
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
        Self { tabs: Vec::new(), active: 0, visible: false, focus_next: false, id: egui::Id::new("strata-terminal"), theme: None }
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

    pub fn active_terminal(&self) -> Option<&Terminal> {
        self.tabs.get(self.active)
    }

    /// Process emulator events; close tabs whose shell exited. True when the
    /// last one just closed (the panel hides itself).
    pub fn poll(&mut self, ctx: &egui::Context) -> bool {
        if self.tabs.is_empty() {
            return false;
        }
        let theme = self.theme.clone().unwrap_or_else(|| Theme::new(Color32::BLACK, Color32::LIGHT_GRAY, Color32::WHITE, Color32::DARK_GRAY));
        for t in &mut self.tabs {
            t.poll(ctx, &theme);
        }
        let mut i = self.tabs.len();
        while i > 0 {
            i -= 1;
            if self.tabs[i].exited {
                self.tabs.remove(i);
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
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let mut close = None;
            for (i, t) in self.tabs.iter().enumerate() {
                let ringing = now - t.bell < 0.25;
                let mut text = egui::RichText::new(t.title()).small();
                if ringing {
                    text = text.color(ui.visuals().warn_fg_color);
                }
                if ui.selectable_label(i == self.active, text).on_hover_text(t.title()).clicked() {
                    self.active = i;
                    self.focus_next = true;
                }
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
            if let Some(i) = close {
                self.tabs.remove(i);
                if self.active > i || self.active >= self.tabs.len() {
                    self.active = self.active.saturating_sub(1);
                }
                if self.tabs.is_empty() {
                    action = Some(PanelAction::Hide);
                }
            }
        });
        let rect = ui.available_rect_before_wrap();
        let resp = ui.interact(rect, self.id, Sense::click_and_drag());
        ui.allocate_rect(rect, Sense::hover());
        ui.painter().rect_filled(rect, 0.0, theme.bg);
        let Some(t) = self.tabs.get_mut(self.active) else {
            return action;
        };
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
            keyboard(ui, t);
        }
        mouse(ui, t, &resp, area.min, cell);
        paint(ui, t, theme, &font, area.min, cell, focused);
        if now - t.bell < 0.25 {
            ui.painter().rect_stroke(rect.shrink(1.0), 0.0, Stroke::new(2.0, theme.ansi[3].gamma_multiply(0.6)), egui::StrokeKind::Inside);
        }
        action
    }
}

/// Turn this frame's keyboard input into PTY bytes, and consume it.
fn keyboard(ui: &egui::Ui, t: &Terminal) {
    let (events, mods) = ui.input(|i| (i.events.clone(), i.modifiers));
    let app_cursor = t.term.lock().mode().contains(TermMode::APP_CURSOR);
    // Option/Alt sends ESC + key (from the Key event), so skip the composed text.
    let meta = mods.alt && !mods.ctrl && !mods.mac_cmd;
    let mut out = Vec::new();
    for e in &events {
        match e {
            Event::Text(text) if !meta => out.extend_from_slice(text.as_bytes()),
            Event::Ime(egui::ImeEvent::Commit(text)) => out.extend_from_slice(text.as_bytes()),
            Event::Key { key, pressed: true, modifiers, .. } => {
                if MAC && modifiers.mac_cmd && *key == Key::K && !modifiers.shift && !modifiers.alt {
                    t.clear();
                } else if modifiers.shift && !modifiers.ctrl && matches!(key, Key::PageUp | Key::PageDown) {
                    // Shift+PgUp/PgDn page through the scrollback.
                    t.term.lock().scroll_display(if *key == Key::PageUp { Scroll::PageUp } else { Scroll::PageDown });
                } else if let Some(b) = key_bytes(*key, *modifiers, app_cursor) {
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

/// Selection, focus clicks, and the scroll wheel.
fn mouse(ui: &egui::Ui, t: &mut Terminal, resp: &egui::Response, origin: egui::Pos2, cell: (f32, f32)) {
    let to_point = |p: egui::Pos2, offset: usize| -> (Point, Side) {
        let x = ((p.x - origin.x) / cell.0).max(0.0);
        let col = (x.floor() as usize).min(t.size.cols - 1);
        let row = (((p.y - origin.y) / cell.1).floor().max(0.0) as usize).min(t.size.rows - 1);
        let side = if x.fract() < 0.5 { Side::Left } else { Side::Right };
        (Point::new(Line(row as i32 - offset as i32), Column(col)), side)
    };
    let offset = t.term.lock().grid().display_offset();
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
    }
    if let Some(pos) = resp.interact_pointer_pos() {
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
        } else if resp.drag_started() {
            let start = ui.input(|i| i.pointer.press_origin()).unwrap_or(pos);
            let (p, side) = to_point(start, offset);
            t.term.lock().selection = Some(Selection::new(SelectionType::Simple, p, side));
        } else if resp.dragged() {
            let (p, side) = to_point(pos, offset);
            if let Some(sel) = t.term.lock().selection.as_mut() {
                sel.update(p, side);
            }
        } else if resp.clicked() {
            t.term.lock().selection = None;
        }
    }

    if !resp.hovered() {
        return;
    }
    let dy = ui.input(|i| i.smooth_scroll_delta.y);
    if dy == 0.0 {
        return;
    }
    t.scroll_acc += dy / cell.1;
    let lines = t.scroll_acc.trunc() as i32;
    if lines == 0 {
        return;
    }
    t.scroll_acc -= lines as f32;
    let mode = *t.term.lock().mode();
    let mouse_mode = mode.intersects(TermMode::MOUSE_REPORT_CLICK | TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION);
    if mouse_mode && mode.contains(TermMode::SGR_MOUSE) {
        let (p, _) = to_point(ui.input(|i| i.pointer.hover_pos()).unwrap_or(origin), 0);
        let button = if lines > 0 { 64 } else { 65 };
        let one = format!("\x1b[<{button};{};{}M", p.column.0 + 1, p.line.0 + 1);
        t.write(one.repeat(lines.unsigned_abs() as usize));
    } else if mode.contains(TermMode::ALT_SCREEN) && mode.contains(TermMode::ALTERNATE_SCROLL) {
        // Full-screen programs (less, vim…) scroll with arrow keys.
        let key = if lines > 0 { Key::ArrowUp } else { Key::ArrowDown };
        let one = key_bytes(key, Modifiers::NONE, mode.contains(TermMode::APP_CURSOR)).unwrap_or_default();
        t.write(one.repeat(lines.unsigned_abs() as usize));
    } else {
        t.term.lock().scroll_display(Scroll::Delta(lines));
    }
}

/// Paint the grid: backgrounds, then text in runs of same-styled cells, then the cursor.
fn paint(ui: &egui::Ui, t: &Terminal, theme: &Theme, font: &FontId, origin: egui::Pos2, cell: (f32, f32), focused: bool) {
    let (cols, rows) = (t.size.cols, t.size.rows);
    let blank = Cell { c: ' ', extra: None, fg: theme.fg, bg: theme.bg, flags: Flags::empty() };
    let mut grid = vec![blank; cols * rows];
    let (cursor, cursor_shape) = {
        let term = t.term.lock();
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
            let extra = ic.cell.zerowidth().map(|z| z.iter().collect::<String>().into_boxed_str());
            grid[vp.line * cols + vp.column.0] = Cell { c: ic.cell.c, extra, fg, bg, flags };
        }
        let c = content.cursor;
        (alacritty_terminal::term::point_to_viewport(offset, c.point), c.shape)
    };
    let painter = ui.painter();
    let x_of = |col: usize| origin.x + col as f32 * cell.0;
    let y_of = |row: usize| origin.y + row as f32 * cell.1;

    // The cursor cell is drawn inverted when it's a focused block.
    let cursor_cell = cursor.filter(|p| p.line < rows && p.column.0 < cols).map(|p| (p.line, p.column.0));
    if let (Some((r, c)), true, CursorShape::Block) = (cursor_cell, focused, cursor_shape) {
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
        // Text: ASCII runs share a galley; anything else is placed cell by cell so
        // wide and fallback-font glyphs stay on the grid.
        let style = |c: &Cell| (c.fg, c.flags & (Flags::BOLD | Flags::ITALIC | Flags::ALL_UNDERLINES | Flags::STRIKEOUT));
        let mut col = 0;
        while col < cols {
            let c = &line[col];
            if c.flags.contains(Flags::WIDE_CHAR_SPACER) || (c.c == ' ' && !c.flags.intersects(Flags::ALL_UNDERLINES | Flags::STRIKEOUT)) {
                col += 1;
                continue;
            }
            let start = col;
            let mut text = String::new();
            if c.c.is_ascii() && c.extra.is_none() {
                let s = style(c);
                while col < cols && line[col].c.is_ascii() && line[col].extra.is_none() && style(&line[col]) == s {
                    text.push(line[col].c);
                    col += 1;
                }
            } else {
                text.push(c.c);
                if let Some(e) = &c.extra {
                    text.push_str(e);
                }
                col += if c.flags.contains(Flags::WIDE_CHAR) { 2 } else { 1 };
            }
            if !c.c.is_ascii() {
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

    if let Some((r, c)) = cursor_cell {
        let wide = grid[r * cols + c].flags.contains(Flags::WIDE_CHAR);
        let w = if wide { cell.0 * 2.0 } else { cell.0 };
        let r = Rect::from_min_size(pos2(x_of(c), y_of(r)), vec2(w, cell.1));
        match (focused, cursor_shape) {
            (_, CursorShape::Hidden) => {}
            (true, CursorShape::Block) => {}
            (true, CursorShape::Beam) => {
                painter.rect_filled(Rect::from_min_size(r.min, vec2(2.0, cell.1)), 0.0, theme.cursor);
            }
            (true, CursorShape::Underline) => {
                painter.rect_filled(Rect::from_min_max(pos2(r.left(), r.bottom() - 2.0), r.max), 0.0, theme.cursor);
            }
            _ => {
                painter.rect_stroke(r.shrink(0.5), 0.0, Stroke::new(1.0, theme.cursor), egui::StrokeKind::Inside);
            }
        }
    }
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
        key_bytes(key, m, false).unwrap_or_default()
    }

    #[test]
    fn special_keys_send_vt_sequences() {
        assert_eq!(bytes(Key::Enter, Modifiers::NONE), b"\r");
        assert_eq!(bytes(Key::Backspace, Modifiers::NONE), b"\x7f");
        assert_eq!(bytes(Key::Tab, Modifiers::NONE), b"\t");
        assert_eq!(bytes(Key::Tab, Modifiers::SHIFT), b"\x1b[Z");
        assert_eq!(bytes(Key::Escape, Modifiers::NONE), b"\x1b");
        assert_eq!(bytes(Key::ArrowUp, Modifiers::NONE), b"\x1b[A");
        assert_eq!(key_bytes(Key::ArrowUp, Modifiers::NONE, true).unwrap(), b"\x1bOA", "DECCKM");
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
        assert_eq!(key_bytes(Key::A, Modifiers::NONE, false), None);
        if MAC {
            assert_eq!(key_bytes(Key::C, Modifiers::MAC_CMD | Modifiers::COMMAND, false), None, "Cmd keys belong to the app");
        }
    }

    #[test]
    fn paste_is_bracketed_on_request() {
        assert_eq!(paste_bytes("a\nb", false), b"a\rb");
        assert_eq!(paste_bytes("a\r\nb", false), b"a\rb");
        assert_eq!(paste_bytes("x\x1b[201~y", true), b"\x1b[200~xy\x1b[201~");
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
        let alive = |pid: u32| std::process::Command::new("kill").args(["-0", &pid.to_string()]).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while alive(pid) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(!alive(pid), "shell {pid} survived its terminal");
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
}
