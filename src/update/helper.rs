//! The relaunch helper. When Strata quits to finish an update, it starts the
//! *previous* version's program (now the backup) as
//! `strata --strata-update-helper <pid> <target> <backup> <version>`, with no
//! window. The helper waits for that process to exit (so the new version
//! finds the session saved and free), starts the new version, and waits for
//! it to show its first window. If the new version exits before that, the
//! helper puts the previous version back, starts it, and leaves a note for
//! it to show. It never deletes the backup itself: the new version does once
//! it's running (see `Updater::startup`).

use super::install;
use super::state::Saved;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

pub const FLAG: &str = "--strata-update-helper";
/// How long to wait for the old process to exit, and the new one to start.
const EXIT_WAIT: Duration = Duration::from_secs(120);
const START_WAIT: Duration = Duration::from_secs(120);

/// What the helper needs: who to wait for, what to start, what to restore.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub pid: u32,
    pub target: PathBuf,
    pub backup: PathBuf,
    pub version: String,
}

impl Plan {
    pub fn args(&self) -> Vec<std::ffi::OsString> {
        vec![FLAG.into(), self.pid.to_string().into(), self.target.clone().into(), self.backup.clone().into(), self.version.clone().into()]
    }

    pub fn parse(args: &[std::ffi::OsString]) -> Option<Self> {
        match args {
            [flag, pid, target, backup, version] if flag == FLAG => Some(Self { pid: pid.to_str()?.parse().ok()?, target: target.into(), backup: backup.into(), version: version.to_str()?.to_owned() }),
            _ => None,
        }
    }
}

/// Called first thing in `main`: if this process is a helper, do the job.
pub fn run_if_helper() -> Option<ExitCode> {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    if args.first().is_none_or(|a| a != FLAG) {
        return None;
    }
    let Some(plan) = Plan::parse(&args) else { return Some(ExitCode::from(2)) };
    let Some(config) = crate::session::config_dir() else { return Some(ExitCode::FAILURE) };
    Some(match finish(&plan, &config, &Real) {
        Outcome::Started | Outcome::StillStarting => ExitCode::SUCCESS,
        Outcome::RolledBack | Outcome::Gone => ExitCode::FAILURE,
    })
}

/// The program for a helper: the backup's (the version being replaced,
/// known to work on this machine).
pub fn helper_program(backup: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        let exe = std::env::current_exe().ok().and_then(|e| e.file_name().map(PathBuf::from)).unwrap_or_else(|| "strata".into());
        backup.join("Contents/MacOS").join(exe)
    } else {
        backup.to_path_buf()
    }
}

/// Start the helper for `plan`, detached; the caller then exits.
pub fn spawn(plan: &Plan) -> std::io::Result<()> {
    let mut cmd = Command::new(helper_program(&plan.backup));
    cmd.args(plan.args()).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    cmd.spawn().map(drop)
}

#[derive(Debug, PartialEq)]
pub enum Outcome {
    /// The new version showed its window.
    Started,
    /// It's still running but hasn't shown a window yet; left alone.
    StillStarting,
    /// It exited without starting: the previous version is back.
    RolledBack,
    /// The old process never exited, or there was nothing to start.
    Gone,
}

/// What the helper does to the world (tests pretend).
pub trait World {
    fn alive(&self, pid: u32) -> bool;
    fn launch(&self, app: &Path) -> std::io::Result<Child>;
    fn sleep(&self, d: Duration);
}

struct Real;

impl World for Real {
    fn alive(&self, pid: u32) -> bool {
        process_alive(pid)
    }

    fn launch(&self, app: &Path) -> std::io::Result<Child> {
        launch(app)
    }

    fn sleep(&self, d: Duration) {
        std::thread::sleep(d);
    }
}

pub fn finish(plan: &Plan, config: &Path, world: &dyn World) -> Outcome {
    let start = Instant::now();
    while world.alive(plan.pid) {
        if start.elapsed() > EXIT_WAIT {
            log_line(config, "the old Strata didn't quit; not relaunching");
            return Outcome::Gone;
        }
        world.sleep(Duration::from_millis(100));
    }
    let mut child = match world.launch(&plan.target) {
        Ok(c) => c,
        Err(e) => {
            log_line(config, &format!("couldn't start the new version: {e}"));
            return roll_back(plan, config, world);
        }
    };
    let start = Instant::now();
    loop {
        if launched(config, &plan.version) {
            // macOS: `open -W` is still waiting on the app; it isn't needed.
            if cfg!(target_os = "macos") {
                let _ = child.kill();
                let _ = child.wait();
            }
            return Outcome::Started;
        }
        if let Ok(Some(_)) = child.try_wait() {
            // Exited (or, macOS, the app did): one more look, it may have
            // written the mark just before.
            world.sleep(Duration::from_millis(500));
            if launched(config, &plan.version) {
                return Outcome::Started;
            }
            return roll_back(plan, config, world);
        }
        if start.elapsed() > START_WAIT {
            return Outcome::StillStarting;
        }
        world.sleep(Duration::from_millis(200));
    }
}

fn launched(config: &Path, version: &str) -> bool {
    Saved::load(config).pending.is_none_or(|p| p.version != version || p.launched)
}

fn roll_back(plan: &Plan, config: &Path, world: &dyn World) -> Outcome {
    let from = Saved::load(config).pending.map(|p| p.from_version).unwrap_or_default();
    let note = match install::restore(&plan.target, &plan.backup) {
        Ok(()) => format!("Strata {} didn't start, so Strata {from} was put back. Please report this at https://github.com/{}/issues.", plan.version, super::manifest::REPO),
        Err(e) => format!("Strata {} didn't start, and the previous version couldn't be put back: {e}", plan.version),
    };
    log_line(config, &note);
    let _ = Saved::update(config, |s| {
        s.pending = None;
        s.notice = Some(note);
    });
    let _ = world.launch(&plan.target).map(|mut c| {
        // Reap it in the background on macOS (`open` returns at once without -W).
        std::thread::spawn(move || c.wait());
    });
    Outcome::RolledBack
}

/// Start the app the way the user would.
fn launch(app: &Path) -> std::io::Result<Child> {
    if cfg!(target_os = "macos") {
        let mut cmd = Command::new("/usr/bin/open");
        // -W: return when the app exits, so a crash on start is noticed.
        cmd.args(["-n", "-W"]);
        // Launch Services doesn't pass the environment on.
        if let Some(dir) = std::env::var_os("STRATA_CONFIG_DIR") {
            let mut kv = std::ffi::OsString::from("STRATA_CONFIG_DIR=");
            kv.push(dir);
            cmd.arg("--env").arg(kv);
        }
        cmd.arg(app).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()
    } else {
        Command::new(app).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()
    }
}

fn log_line(config: &Path, msg: &str) {
    use std::io::Write;
    let log = crate::logging::log_dir(config).join("update-helper.log");
    let _ = crate::private::create_dir_all(log.parent().unwrap_or(config));
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&log) {
        let _ = writeln!(f, "{} {msg}", super::state::now_secs());
    }
}

#[cfg(unix)]
pub fn process_alive(pid: u32) -> bool {
    // Signal 0 checks without sending; EPERM still means it exists.
    let r = unsafe { libc::kill(pid as libc::pid_t, 0) };
    r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(windows)]
pub fn process_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
    // SAFETY: plain Win32 calls on a handle we own and close.
    unsafe {
        let h = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if h.is_null() {
            return false;
        }
        let r = WaitForSingleObject(h, 0);
        CloseHandle(h);
        r == WAIT_TIMEOUT
    }
}

#[cfg(not(any(unix, windows)))]
pub fn process_alive(_pid: u32) -> bool {
    false
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::super::state::Pending;
    use super::*;
    #[cfg(unix)]
    use std::cell::Cell;

    #[cfg(unix)] // used by the unix-only tests below
    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("strata-helper-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[cfg(unix)]
    /// The old process is alive for `alive_for` checks; "launching" runs a
    /// shell command standing in for the new version.
    struct Fake {
        alive_for: Cell<u32>,
        script: String,
    }

    #[cfg(unix)]
    impl World for Fake {
        fn alive(&self, _pid: u32) -> bool {
            let n = self.alive_for.get();
            self.alive_for.set(n.saturating_sub(1));
            n > 0
        }
        fn launch(&self, _app: &Path) -> std::io::Result<Child> {
            Command::new("/bin/sh").args(["-c", &self.script]).spawn()
        }
        fn sleep(&self, d: Duration) {
            std::thread::sleep(d.min(Duration::from_millis(20)));
        }
    }

    #[cfg(unix)]
    fn setup(tag: &str) -> (PathBuf, Plan) {
        let dir = temp(tag);
        let target = dir.join("Strata.app");
        let backup = dir.join(".Strata.app.old");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("v"), "new").unwrap();
        std::fs::create_dir_all(&backup).unwrap();
        std::fs::write(backup.join("v"), "old").unwrap();
        let config = dir.join("config");
        Saved::update(&config, |s| s.pending = Some(Pending { version: "1.1.0".into(), from_version: "1.0.0".into(), target: target.clone(), backup: backup.clone(), launched: false })).unwrap();
        (config, Plan { pid: 1, target, backup, version: "1.1.0".into() })
    }

    #[test]
    fn plan_round_trips_through_arguments() {
        let plan = Plan { pid: 42, target: "/A/Strata.app".into(), backup: "/A/.Strata.app.old".into(), version: "1.2.0".into() };
        assert_eq!(Plan::parse(&plan.args()), Some(plan.clone()));
        assert_eq!(Plan::parse(&plan.args()[..3]), None);
        assert!(helper_program(&plan.backup).starts_with(&plan.backup), "the helper is the old version");
    }

    #[cfg(unix)]
    #[test]
    fn a_new_version_that_starts_is_kept() {
        let (config, plan) = setup("ok");
        // "Starts": marks the launch in update.json, as the new version does.
        let mut pending = Saved::load(&config).pending.unwrap();
        pending.launched = true;
        let json = serde_json::to_string(&Saved { pending: Some(pending), ..Default::default() }).unwrap();
        let script = format!("printf '%s' '{json}' > '{}'; sleep 1", Saved::path(&config).display());
        let outcome = finish(&plan, &config, &Fake { alive_for: Cell::new(3), script });
        assert_eq!(outcome, Outcome::Started);
        assert_eq!(std::fs::read_to_string(plan.target.join("v")).unwrap(), "new");
        assert!(plan.backup.exists(), "the new version deletes the backup itself");
    }

    #[cfg(unix)]
    #[test]
    fn a_new_version_that_dies_is_rolled_back() {
        let (config, plan) = setup("crash");
        let outcome = finish(&plan, &config, &Fake { alive_for: Cell::new(0), script: "exit 1".into() });
        assert_eq!(outcome, Outcome::RolledBack);
        assert_eq!(std::fs::read_to_string(plan.target.join("v")).unwrap(), "old");
        assert!(!plan.backup.exists());
        let saved = Saved::load(&config);
        assert!(saved.pending.is_none());
        assert!(saved.notice.unwrap().contains("1.1.0 didn't start"));
    }

    #[test]
    fn only_this_process_is_alive() {
        assert!(process_alive(std::process::id()));
        assert!(!process_alive(u32::MAX / 2));
    }
}
