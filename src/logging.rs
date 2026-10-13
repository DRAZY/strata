//! Strata's log: a small `log` backend that appends to
//! `<config dir>/Logs/strata.log` and rotates it (1 MB, three files), keeps
//! the last lines in memory for crash reports and Help ▸ Report a Problem,
//! and copies them to stderr when Strata runs in a terminal.
//!
//! Strata's own messages are logged at `info` and up by default; other crates
//! (eframe, wgpu, winit…) only at `warn` and up. `STRATA_LOG=debug` raises
//! Strata's level; `STRATA_LOG=debug,wgpu=info` also sets one crate's.
//!
//! Only paths, errors and events are logged: never file contents, clipboard
//! text or anything typed. Nothing leaves the machine.

use std::collections::VecDeque;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, TryLockError};
use std::time::SystemTime;

pub const FILE_NAME: &str = "strata.log";
/// Rotate when the log reaches this size.
pub const MAX_BYTES: u64 = 1024 * 1024;
/// strata.log plus this many older ones (strata.1.log, strata.2.log).
pub const KEEP_OLD: usize = 2;
/// Lines kept in memory for crash reports.
pub const TAIL_LINES: usize = 200;

/// Where log files go.
pub fn log_dir(config: &Path) -> PathBuf {
    config.join("Logs")
}

/// Levels per target: Strata's own, one for everything else, and overrides.
#[derive(Clone, Debug, PartialEq)]
pub struct Filter {
    strata: log::LevelFilter,
    others: log::LevelFilter,
    targets: Vec<(String, log::LevelFilter)>,
}

impl Default for Filter {
    fn default() -> Self {
        Self { strata: log::LevelFilter::Info, others: log::LevelFilter::Warn, targets: Vec::new() }
    }
}

impl Filter {
    /// `STRATA_LOG`'s format: a level for Strata ("debug"), and/or
    /// `target=level` pairs, comma-separated. Unknown parts are ignored.
    pub fn parse(spec: &str) -> Self {
        let mut f = Self::default();
        for part in spec.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            match part.split_once('=') {
                Some((target, level)) => {
                    if let Ok(level) = level.trim().parse() {
                        f.targets.push((target.trim().to_owned(), level));
                    }
                }
                None => {
                    if let Ok(level) = part.parse() {
                        f.strata = level;
                    }
                }
            }
        }
        f
    }

    fn level_for(&self, target: &str) -> log::LevelFilter {
        let matches = |prefix: &str| target == prefix || target.strip_prefix(prefix).is_some_and(|r| r.starts_with("::"));
        if let Some((_, l)) = self.targets.iter().rev().find(|(t, _)| matches(t)) {
            return *l;
        }
        if matches("strata") { self.strata } else { self.others }
    }

    pub fn enabled(&self, target: &str, level: log::Level) -> bool {
        level <= self.level_for(target)
    }

    fn max(&self) -> log::LevelFilter {
        self.targets.iter().map(|(_, l)| *l).chain([self.strata, self.others]).max().unwrap_or(log::LevelFilter::Off)
    }
}

/// The log file and its rotation.
pub struct Sink {
    dir: Option<PathBuf>,
    file: Option<File>,
    size: u64,
    max_bytes: u64,
    /// Opening failed: don't retry on every line.
    broken: bool,
}

impl Sink {
    pub fn new(dir: Option<PathBuf>, max_bytes: u64) -> Self {
        Self { dir, file: None, size: 0, max_bytes, broken: false }
    }

    fn path(dir: &Path, n: usize) -> PathBuf {
        if n == 0 { dir.join(FILE_NAME) } else { dir.join(format!("strata.{n}.log")) }
    }

    fn open(&mut self) -> Option<&mut File> {
        if self.file.is_none() && !self.broken {
            let dir = self.dir.as_ref()?;
            let opened = std::fs::create_dir_all(dir).and_then(|()| std::fs::OpenOptions::new().create(true).append(true).open(Self::path(dir, 0)));
            match opened {
                Ok(f) => {
                    self.size = f.metadata().map_or(0, |m| m.len());
                    self.file = Some(f);
                }
                Err(_) => self.broken = true,
            }
        }
        self.file.as_mut()
    }

    /// strata.log → strata.1.log → strata.2.log; the oldest is dropped.
    fn rotate(&mut self) {
        let Some(dir) = self.dir.clone() else { return };
        self.file = None;
        let _ = std::fs::remove_file(Self::path(&dir, KEEP_OLD));
        for n in (0..KEEP_OLD).rev() {
            let _ = std::fs::rename(Self::path(&dir, n), Self::path(&dir, n + 1));
        }
        self.size = 0;
    }

    pub fn write_line(&mut self, line: &str) {
        if self.size > 0 && self.size + line.len() as u64 + 1 > self.max_bytes {
            self.rotate();
        }
        // One write per line, so lines from a second Strata don't interleave.
        let bytes = format!("{line}\n");
        if self.open().is_some_and(|f| f.write_all(bytes.as_bytes()).is_ok()) {
            self.size += bytes.len() as u64;
        }
    }
}

pub struct Logger {
    filter: Filter,
    sink: Mutex<Sink>,
    tail: Mutex<VecDeque<String>>,
    stderr: bool,
}

impl Logger {
    pub fn new(dir: Option<PathBuf>, filter: Filter, stderr: bool) -> Self {
        Self { filter, sink: Mutex::new(Sink::new(dir, MAX_BYTES)), tail: Mutex::new(VecDeque::with_capacity(TAIL_LINES)), stderr }
    }

    fn format(level: log::Level, target: &str, args: &std::fmt::Arguments) -> String {
        let target = target.strip_prefix("strata::").unwrap_or(target);
        format!("{} {level:<5} [{target}] {args}", timestamp(SystemTime::now()))
    }

    fn push_tail(tail: &mut VecDeque<String>, line: &str) {
        if tail.len() == TAIL_LINES {
            tail.pop_front();
        }
        tail.push_back(line.to_owned());
    }

    fn emit(&self, line: &str, blocking: bool) {
        // From the panic hook, a lock held by the panicking thread must not
        // deadlock us: skip what's busy instead.
        fn get<T>(m: &Mutex<T>, blocking: bool) -> Option<std::sync::MutexGuard<'_, T>> {
            if blocking {
                return Some(m.lock().unwrap_or_else(|e| e.into_inner()));
            }
            match m.try_lock() {
                Ok(g) => Some(g),
                Err(TryLockError::Poisoned(e)) => Some(e.into_inner()),
                Err(TryLockError::WouldBlock) => None,
            }
        }
        if let Some(mut tail) = get(&self.tail, blocking) {
            Self::push_tail(&mut tail, line);
        }
        if let Some(mut sink) = get(&self.sink, blocking) {
            sink.write_line(line);
        }
        if self.stderr {
            let _ = writeln!(std::io::stderr(), "strata: {line}");
        }
    }

    /// The last lines logged, oldest first. `None` if the buffer is busy.
    fn tail(&self, blocking: bool) -> Option<Vec<String>> {
        let g = if blocking {
            self.tail.lock().unwrap_or_else(|e| e.into_inner())
        } else {
            match self.tail.try_lock() {
                Ok(g) => g,
                Err(TryLockError::Poisoned(e)) => e.into_inner(),
                Err(TryLockError::WouldBlock) => return None,
            }
        };
        Some(g.iter().cloned().collect())
    }
}

impl log::Log for Logger {
    fn enabled(&self, m: &log::Metadata) -> bool {
        self.filter.enabled(m.target(), m.level())
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            self.emit(&Self::format(record.level(), record.target(), record.args()), true);
        }
    }

    fn flush(&self) {}
}

static LOGGER: OnceLock<Logger> = OnceLock::new();

/// Start logging to `<config>/Logs` (memory only without a config folder).
/// Call once, early; later calls do nothing.
pub fn init(config: Option<&Path>) {
    let filter = std::env::var("STRATA_LOG").map(|s| Filter::parse(&s)).unwrap_or_default();
    let stderr = std::io::IsTerminal::is_terminal(&std::io::stderr());
    install(Logger::new(config.map(log_dir), filter, stderr));
}

fn install(logger: Logger) {
    let max = logger.filter.max();
    if LOGGER.set(logger).is_ok()
        && let Some(l) = LOGGER.get()
        && log::set_logger(l).is_ok()
    {
        log::set_max_level(max);
    }
}

/// Tests share one in-memory logger (no file).
#[cfg(test)]
pub fn init_for_tests() {
    install(Logger::new(None, Filter::parse("debug"), false));
}

/// The last lines logged, oldest first.
pub fn recent_lines() -> Vec<String> {
    LOGGER.get().and_then(|l| l.tail(true)).unwrap_or_default()
}

/// For the panic hook: never blocks; `None` if another thread holds the lock.
pub fn try_recent_lines() -> Option<Vec<String>> {
    LOGGER.get().map_or(Some(Vec::new()), |l| l.tail(false))
}

/// Log without blocking (the panic hook): lines that can't get the lock are dropped.
pub fn try_log(level: log::Level, message: &str) {
    if let Some(l) = LOGGER.get()
        && l.filter.enabled("strata", level)
    {
        l.emit(&Logger::format(level, "strata::crash", &format_args!("{message}")), false);
    }
}

/// Days since 1970-01-01 → (year, month, day), proleptic Gregorian.
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

fn parts(t: SystemTime) -> (i64, u32, u32, u64, u64, u64, u32) {
    let d = t.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default();
    let secs = d.as_secs();
    let (y, mo, day) = civil((secs / 86_400) as i64);
    let s = secs % 86_400;
    (y, mo, day, s / 3600, s / 60 % 60, s % 60, d.subsec_millis())
}

/// `2026-10-13T09:41:07.123Z` (UTC).
pub fn timestamp(t: SystemTime) -> String {
    let (y, mo, d, h, mi, s, ms) = parts(t);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{ms:03}Z")
}

/// `20261013-094107` (UTC), for file names.
pub fn file_stamp(t: SystemTime) -> String {
    let (y, mo, d, h, mi, s, _) = parts(t);
    format!("{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("strata-log-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn rotates_at_the_size_limit_keeping_three_files() {
        let dir = temp_dir("rotate");
        let mut sink = Sink::new(Some(dir.clone()), 100);
        // 30 lines of 20 bytes: 600 bytes through a 100-byte limit.
        for i in 0..30 {
            sink.write_line(&format!("line {i:02} -----------"));
        }
        let mut names: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        assert_eq!(names, ["strata.1.log", "strata.2.log", "strata.log"]);
        for n in &names {
            let len = std::fs::metadata(dir.join(n)).unwrap().len();
            assert!(len <= 100, "{n} is {len} bytes");
        }
        // Newest lines in strata.log, older ones moved down the chain.
        let newest = std::fs::read_to_string(dir.join("strata.log")).unwrap();
        assert!(newest.ends_with("line 29 -----------\n"), "{newest}");
        let older = std::fs::read_to_string(dir.join("strata.1.log")).unwrap();
        assert!(older.contains("line 2") && !older.contains("line 29"));
        // An existing file's size counts after a restart.
        let mut again = Sink::new(Some(dir.clone()), 100);
        for _ in 0..5 {
            again.write_line("0123456789012345678");
        }
        assert!(std::fs::metadata(dir.join("strata.log")).unwrap().len() <= 100);
        assert_ne!(std::fs::read_to_string(dir.join("strata.1.log")).unwrap(), older, "rotated again, not appended past the limit");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn lines_have_a_timestamp_level_and_target() {
        let dir = temp_dir("format");
        let logger = Logger::new(Some(dir.clone()), Filter::default(), false);
        log::Log::log(&logger, &log::Record::builder().level(log::Level::Warn).target("strata::app").args(format_args!("couldn't open /x: denied")).build());
        log::Log::log(&logger, &log::Record::builder().level(log::Level::Debug).target("strata::app").args(format_args!("hidden at info")).build());
        log::Log::log(&logger, &log::Record::builder().level(log::Level::Info).target("wgpu_core").args(format_args!("noisy")).build());
        let text = std::fs::read_to_string(dir.join(FILE_NAME)).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 1, "{text}");
        let re = regex::Regex::new(r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z WARN  \[app\] couldn't open /x: denied$").unwrap();
        assert!(re.is_match(lines[0]), "{}", lines[0]);
        assert_eq!(logger.tail(false).unwrap(), lines);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn the_memory_tail_keeps_the_last_lines() {
        let logger = Logger::new(None, Filter::default(), false);
        for i in 0..(TAIL_LINES + 50) {
            logger.emit(&format!("n{i}"), true);
        }
        let tail = logger.tail(true).unwrap();
        assert_eq!(tail.len(), TAIL_LINES);
        assert_eq!(tail[0], "n50");
        // A held lock makes the non-blocking reader give up instead of waiting.
        let held = logger.tail.lock().unwrap();
        assert!(logger.tail(false).is_none());
        logger.emit("dropped, not deadlocked", false);
        drop(held);
        assert!(!logger.tail(true).unwrap().iter().any(|l| l.contains("dropped")));
    }

    #[test]
    fn strata_log_sets_levels() {
        let f = Filter::default();
        assert!(f.enabled("strata::app", log::Level::Info) && !f.enabled("strata::app", log::Level::Debug));
        assert!(!f.enabled("wgpu_core::device", log::Level::Info) && f.enabled("wgpu_core", log::Level::Warn));
        let f = Filter::parse("debug, wgpu_core=info,bogus=nope");
        assert!(f.enabled("strata", log::Level::Debug) && !f.enabled("strata", log::Level::Trace));
        assert!(f.enabled("wgpu_core::device", log::Level::Info) && !f.enabled("wgpu_hal", log::Level::Info));
        assert!(!f.enabled("stratagem", log::Level::Debug), "a prefix of a word isn't the crate");
        assert_eq!(f.max(), log::LevelFilter::Debug);
        assert_eq!(Filter::parse("nonsense"), Filter::default());
        assert!(!Filter::parse("off").enabled("strata", log::Level::Error));
    }

    #[test]
    fn utc_timestamps() {
        assert_eq!(timestamp(SystemTime::UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
        let t = SystemTime::UNIX_EPOCH + Duration::from_millis(1_791_884_467_250);
        assert_eq!(timestamp(t), "2026-10-13T09:41:07.250Z");
        assert_eq!(file_stamp(t), "20261013-094107");
        // Leap day.
        assert_eq!(timestamp(SystemTime::UNIX_EPOCH + Duration::from_secs(951_782_400)), "2000-02-29T00:00:00.000Z");
    }
}
