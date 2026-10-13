//! Crash reports, and keeping unsaved work when Strata crashes.
//!
//! A panic on the main (UI) thread is a crash: the panic hook writes
//! `<config dir>/Crashes/strata-crash-<time>.txt` (version, commit, system,
//! the panic, a backtrace and the last log lines). `main` catches the unwind
//! around every frame, saves unsaved tabs (`rescue`: with the session, as hot
//! exit does on quit, or into a `backups.recovered-<time>` folder), adds what
//! it did to the report, and exits. The next launch sees a report newer than
//! the previous launch and offers it (`check_previous`).
//!
//! A panic on any other thread (search, git, builds, the terminal…) only
//! ends that thread: it's logged with its backtrace, and Strata keeps going.
//!
//! Nothing is sent anywhere. Reports are plain text the user can read and
//! choose to share.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, Once, OnceLock};
use std::time::SystemTime;

/// Where problems are reported. TODO(owner): point at the real repository.
pub const ISSUES_URL: &str = "https://github.com/strata-editor/strata/issues";

/// The commit Strata was built from, when built from a git checkout.
pub const COMMIT: Option<&str> = option_env!("STRATA_GIT_HASH");

pub const REPORT_PREFIX: &str = "strata-crash-";
const LAST_LAUNCH: &str = ".last-launch";

pub fn crash_dir(config: &Path) -> PathBuf {
    config.join("Crashes")
}

/// "Strata 0.1.0 (commit 1a2b3c4d5e6f)".
pub fn version_line() -> String {
    match COMMIT {
        Some(c) => format!("Strata {} (commit {c})", env!("CARGO_PKG_VERSION")),
        None => format!("Strata {}", env!("CARGO_PKG_VERSION")),
    }
}

/// "macOS 15.1 (aarch64)".
pub fn system_line() -> String {
    format!("{} ({})", os_version(), std::env::consts::ARCH)
}

fn os_version() -> String {
    #[cfg(target_os = "macos")]
    {
        let plist = std::fs::read_to_string("/System/Library/CoreServices/SystemVersion.plist").unwrap_or_default();
        let version = plist.split("<key>ProductVersion</key>").nth(1).and_then(|r| r.split("<string>").nth(1)).and_then(|r| r.split("</string>").next());
        return format!("macOS {}", version.unwrap_or("(unknown version)"));
    }
    #[cfg(target_os = "linux")]
    {
        let release = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
        let name = release.lines().find_map(|l| l.strip_prefix("PRETTY_NAME=")).map(|n| n.trim_matches('"').to_owned());
        return format!("Linux{}", name.map(|n| format!(", {n}")).unwrap_or_default());
    }
    #[allow(unreachable_code)]
    std::env::consts::OS.to_owned()
}

/// What a panic said and where.
#[derive(Debug, Clone)]
pub struct Panic {
    pub thread: String,
    pub message: String,
    pub location: String,
    pub backtrace: String,
}

impl Panic {
    fn of(info: &std::panic::PanicHookInfo) -> Self {
        Self {
            thread: std::thread::current().name().unwrap_or("unnamed").to_owned(),
            message: payload_text(info.payload()),
            location: info.location().map_or_else(|| "an unknown location".into(), |l| format!("{}:{}:{}", l.file(), l.line(), l.column())),
            backtrace: std::backtrace::Backtrace::force_capture().to_string(),
        }
    }
}

pub fn payload_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload.downcast_ref::<&str>().map(|s| (*s).to_owned()).or_else(|| payload.downcast_ref::<String>().cloned()).unwrap_or_else(|| "(no message)".into())
}

/// A crash report's text. `log` is `None` when the log was busy.
pub fn report_text(p: &Panic, log: Option<&[String]>, at: SystemTime) -> String {
    let mut s = String::new();
    s.push_str("Strata crash report\n\n");
    s.push_str(&format!("Version: {}\n", version_line()));
    s.push_str(&format!("System: {}\n", system_line()));
    s.push_str(&format!("Time: {}\n", crate::logging::timestamp(at)));
    s.push_str(&format!("Thread: {}\n", p.thread));
    s.push_str(&format!("Panic: {}\n", p.message));
    s.push_str(&format!("Location: {}\n", p.location));
    s.push_str("\nBacktrace:\n");
    s.push_str(p.backtrace.trim_end());
    s.push_str("\n\nRecent log:\n");
    match log {
        Some([]) => s.push_str("(empty)\n"),
        Some(lines) => lines.iter().for_each(|l| {
            s.push_str(l);
            s.push('\n');
        }),
        None => s.push_str("(the log was busy)\n"),
    }
    s.push('\n');
    s
}

/// What happened to unsaved work after a crash.
#[derive(Debug, Clone, PartialEq)]
pub enum Rescue {
    /// Saved with the session (hot exit); `tabs` of them unsaved.
    Session { tabs: usize },
    /// Written as text files into this folder.
    Folder { path: PathBuf, tabs: usize },
    /// Nothing was unsaved.
    Nothing,
    Failed(String),
}

impl Rescue {
    /// The lines added to the crash report: one to read, one for the next launch.
    pub fn report_lines(&self) -> String {
        let plural = |n: usize| if n == 1 { "1 unsaved tab".to_owned() } else { format!("{n} unsaved tabs") };
        let (human, key) = match self {
            Rescue::Session { tabs: 0 } => ("nothing unsaved; the session was saved".to_owned(), "session".to_owned()),
            Rescue::Session { tabs } => (format!("{} saved with the session; they reopen at the next launch", plural(*tabs)), "session".into()),
            Rescue::Folder { path, tabs } => (format!("{} saved to {}", plural(*tabs), path.display()), format!("folder {}", path.display())),
            Rescue::Nothing => ("nothing unsaved".into(), "none".into()),
            Rescue::Failed(e) => (format!("couldn't be saved ({e}); the last autosave has it up to a few seconds earlier"), "failed".into()),
        };
        format!("Unsaved work: {human}\nRecovery: {key}\n")
    }
}

/// Write unsaved texts as `.txt` files into a new `backups.recovered-<time>`
/// folder next to the session (hot exit off, or the session couldn't be saved).
pub fn write_recovered(config: &Path, texts: &[(String, String)]) -> std::io::Result<PathBuf> {
    let stamp = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let dir = config.join(format!("backups.recovered-{stamp}"));
    std::fs::create_dir_all(&dir)?;
    for (i, (name, text)) in texts.iter().enumerate() {
        let safe: String = name.chars().map(|c| if c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | ' ') { c } else { '_' }).take(60).collect();
        std::fs::write(dir.join(format!("{:02} {}.txt", i + 1, safe.trim())), text)?;
    }
    Ok(dir)
}

struct Reporter {
    dir: Option<PathBuf>,
}

static REPORTER: OnceLock<Reporter> = OnceLock::new();
/// The main thread has panicked (a crash report was or is being written).
static CRASHED: AtomicBool = AtomicBool::new(false);
static LAST_REPORT: Mutex<Option<PathBuf>> = Mutex::new(None);

#[cfg(test)]
thread_local! {
    /// Tests: treat a panic on this thread as a crash, reporting into this folder.
    static TEST_DIR: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
    static TEST_REPORT: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Install the panic hook, reporting crashes into `<config>/Crashes`.
pub fn install(config: Option<&Path>) {
    let _ = REPORTER.set(Reporter { dir: config.map(crash_dir) });
    install_hook();
}

/// The hook runs before the default one (which prints to stderr), for every
/// thread. Installed once; without a `Reporter` (tests) it only logs.
fn install_hook() {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            on_panic(info);
            previous(info);
        }));
    });
}

fn on_panic(info: &std::panic::PanicHookInfo) {
    let p = Panic::of(info);
    #[cfg(test)]
    if let Some(dir) = TEST_DIR.with(|d| d.borrow().clone()) {
        let path = write_report(&dir, &p);
        TEST_REPORT.with(|r| *r.borrow_mut() = path);
        return;
    }
    let reporter = REPORTER.get().filter(|_| p.thread == "main");
    let Some(reporter) = reporter else {
        crate::logging::try_log(log::Level::Error, &format!("thread '{}' panicked at {}: {}\n{}", p.thread, p.location, p.message, p.backtrace.trim_end()));
        return;
    };
    if CRASHED.swap(true, Ordering::SeqCst) {
        crate::logging::try_log(log::Level::Error, &format!("panicked again while handling the crash, at {}: {}", p.location, p.message));
        return;
    }
    crate::logging::try_log(log::Level::Error, &format!("Strata crashed: panic at {}: {}", p.location, p.message));
    if let Some(dir) = &reporter.dir
        && let Some(path) = write_report(dir, &p)
        && let Ok(mut last) = LAST_REPORT.try_lock()
    {
        *last = Some(path);
    }
}

fn write_report(dir: &Path, p: &Panic) -> Option<PathBuf> {
    let now = SystemTime::now();
    let text = report_text(p, crate::logging::try_recent_lines().as_deref(), now);
    std::fs::create_dir_all(dir).ok()?;
    // Two crashes in one second (two processes) get their own files.
    let stamp = crate::logging::file_stamp(now);
    let path = (0..100).map(|n| dir.join(if n == 0 { format!("{REPORT_PREFIX}{stamp}.txt") } else { format!("{REPORT_PREFIX}{stamp}-{n}.txt") })).find(|p| !p.exists())?;
    std::fs::write(&path, text).ok()?;
    Some(path)
}

/// A crash happened in this process (the hook saw the main thread panic).
pub fn crashed() -> bool {
    CRASHED.load(Ordering::SeqCst)
}

/// Run the emergency save; a panic inside it becomes `Rescue::Failed`.
pub fn rescue(save: impl FnOnce() -> Rescue) -> Rescue {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(save)).unwrap_or_else(|p| Rescue::Failed(format!("the save panicked too: {}", payload_text(p.as_ref()))))
}

/// Record what the emergency save did, in the log and the crash report.
pub fn finish(rescue: &Rescue) {
    let lines = rescue.report_lines();
    crate::logging::try_log(log::Level::Error, lines.lines().next().unwrap_or_default());
    let report = LAST_REPORT.try_lock().ok().and_then(|g| g.clone());
    if let Some(path) = report {
        append(&path, &lines);
    }
}

fn append(path: &Path, text: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(path) {
        let _ = f.write_all(text.as_bytes());
    }
}

/// The last launch crashed: its report and what happened to unsaved work.
#[derive(Debug, Clone, PartialEq)]
pub struct PreviousCrash {
    pub report: PathBuf,
    /// The report's "Recovery:" value (empty when the save never ran).
    pub recovery: String,
}

impl PreviousCrash {
    /// The second sentence of the notice.
    pub fn summary(&self) -> String {
        match self.recovery.split_once(' ').map_or((self.recovery.as_str(), ""), |(a, b)| (a, b)) {
            ("session", _) => "Your unsaved work was restored.".into(),
            ("folder", path) => format!("Your unsaved text was saved to {path}."),
            ("none", _) => "Nothing unsaved was lost.".into(),
            _ => "Anything unsaved since the last autosave (a few seconds before) may be missing.".into(),
        }
    }
}

/// At launch: the newest crash report written since the previous launch, if
/// any. Remembers this launch, so each report is offered once.
pub fn check_previous(config: &Path) -> Option<PreviousCrash> {
    let dir = crash_dir(config);
    let marker = dir.join(LAST_LAUNCH);
    let last = std::fs::read_to_string(&marker).ok().and_then(|s| s.trim().parse::<u128>().ok());
    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    if std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&marker, now.to_string())).is_err() {
        log::warn!("couldn't write {}", marker.display());
    }
    let last = SystemTime::UNIX_EPOCH + std::time::Duration::from_nanos(u64::try_from(last?).ok()?);
    let newest = std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(REPORT_PREFIX))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .filter(|(t, _)| *t > last)
        .max()?
        .1;
    let text = std::fs::read_to_string(&newest).unwrap_or_default();
    let recovery = text.lines().rev().find_map(|l| l.strip_prefix("Recovery: ")).unwrap_or_default().to_owned();
    Some(PreviousCrash { report: newest, recovery })
}

/// The newest crash report, if there is one.
pub fn latest_report(config: &Path) -> Option<PathBuf> {
    std::fs::read_dir(crash_dir(config))
        .ok()?
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(REPORT_PREFIX))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .max()
        .map(|(_, p)| p)
}

/// Dev aid: `STRATA_DEV_PANIC=40` panics on the UI thread at frame 40;
/// `thread:40` panics on a background thread instead (Strata should keep
/// going). Inert unless set.
pub struct DevPanic {
    at: Option<(bool, u64)>,
    frame: u64,
}

impl DevPanic {
    pub fn from_env() -> Self {
        let spec = std::env::var("STRATA_DEV_PANIC").unwrap_or_default();
        let (thread, n) = match spec.strip_prefix("thread:") {
            Some(n) => (true, n),
            None => (false, spec.as_str()),
        };
        Self { at: n.trim().parse().ok().map(|n| (thread, n)), frame: 0 }
    }

    pub fn tick(&mut self, ctx: &egui::Context) {
        let Some((thread, n)) = self.at else { return };
        self.frame += 1;
        ctx.request_repaint();
        if self.frame < n {
            return;
        }
        self.at = None;
        if thread {
            let _ = std::thread::Builder::new().name("dev-panic".into()).spawn(|| panic!("STRATA_DEV_PANIC: deliberate panic on a background thread"));
        } else {
            panic!("STRATA_DEV_PANIC: deliberate crash at frame {n}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("strata-crash-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_panic_writes_a_full_report() {
        crate::logging::init_for_tests();
        install_hook();
        let dir = temp_dir("report");
        let crashes = crash_dir(&dir);
        let worker = crashes.clone();
        let result = std::thread::Builder::new()
            .name("ui-under-test".into())
            .spawn(move || {
                TEST_DIR.with(|d| *d.borrow_mut() = Some(worker));
                log::info!("opened /tmp/some/file.rs");
                let caught = std::panic::catch_unwind(|| {
                    let v: Vec<u8> = Vec::new();
                    v[3]
                });
                assert!(caught.is_err());
                TEST_REPORT.with(|r| r.borrow().clone())
            })
            .unwrap()
            .join()
            .unwrap();
        let path = result.expect("a report was written");
        assert!(path.starts_with(&crashes));
        assert!(path.file_name().unwrap().to_string_lossy().starts_with(REPORT_PREFIX));
        let text = std::fs::read_to_string(&path).unwrap();
        for want in [
            format!("Version: Strata {}", env!("CARGO_PKG_VERSION")),
            format!("System: {}", system_line()),
            "Thread: ui-under-test".into(),
            "Panic: index out of bounds".into(),
            "Location: src/crash.rs:".into(),
            "Backtrace:\n".into(),
            "Recent log:\n".into(),
            "opened /tmp/some/file.rs".into(),
        ] {
            assert!(text.contains(&want), "{want:?} missing from\n{text}");
        }
        // The backtrace names the function that panicked (debug info in tests).
        assert!(text.contains("crash::tests"), "{text}");
        // The emergency save's outcome joins the report.
        append(&path, &Rescue::Session { tabs: 2 }.report_lines());
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("Unsaved work: 2 unsaved tabs saved with the session"));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn background_panics_are_not_crashes() {
        crate::logging::init_for_tests();
        install_hook();
        let joined = std::thread::Builder::new().name("search-worker".into()).spawn(|| panic!("worker gave up 7f3a")).unwrap().join();
        assert!(joined.is_err());
        assert!(!crashed());
        let lines = crate::logging::recent_lines();
        assert!(lines.iter().any(|l| l.contains("thread 'search-worker' panicked") && l.contains("worker gave up 7f3a")), "{lines:?}");
    }

    #[test]
    fn the_next_launch_finds_a_new_report_once() {
        let dir = temp_dir("next");
        // First launch ever: nothing to show.
        assert_eq!(check_previous(&dir), None);
        assert_eq!(check_previous(&dir), None);
        std::thread::sleep(std::time::Duration::from_millis(20));
        let crashes = crash_dir(&dir);
        let report = crashes.join(format!("{REPORT_PREFIX}20261013-094107.txt"));
        std::fs::write(&report, format!("Strata crash report\n…\n{}", Rescue::Session { tabs: 1 }.report_lines())).unwrap();
        let found = check_previous(&dir).expect("the crash is noticed");
        assert_eq!(found.report, report);
        assert_eq!(found.recovery, "session");
        assert_eq!(found.summary(), "Your unsaved work was restored.");
        // Offered once: the launch after that is clean.
        assert_eq!(check_previous(&dir), None);
        assert_eq!(latest_report(&dir), Some(report));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn summaries_follow_the_recovery() {
        let c = |r: &Rescue| {
            let lines = r.report_lines();
            let recovery = lines.lines().find_map(|l| l.strip_prefix("Recovery: ")).unwrap().to_owned();
            PreviousCrash { report: PathBuf::new(), recovery }.summary()
        };
        assert_eq!(c(&Rescue::Folder { path: "/c/backups.recovered-9".into(), tabs: 2 }), "Your unsaved text was saved to /c/backups.recovered-9.");
        assert_eq!(c(&Rescue::Nothing), "Nothing unsaved was lost.");
        assert!(c(&Rescue::Failed("disk full".into())).contains("last autosave"));
        assert!(PreviousCrash { report: PathBuf::new(), recovery: String::new() }.summary().contains("last autosave"));
    }

    #[test]
    fn recovered_texts_are_plain_files() {
        let dir = temp_dir("recovered");
        let out = write_recovered(&dir, &[("notes.md".into(), "draft one".into()), ("a/b:c".into(), "two".into())]).unwrap();
        assert!(out.file_name().unwrap().to_string_lossy().starts_with("backups.recovered-"));
        assert_eq!(std::fs::read_to_string(out.join("01 notes.md.txt")).unwrap(), "draft one");
        assert_eq!(std::fs::read_to_string(out.join("02 a_b_c.txt")).unwrap(), "two");
        assert_eq!(rescue(|| panic!("again")), Rescue::Failed("the save panicked too: again".into()));
        std::fs::remove_dir_all(dir).ok();
    }
}
