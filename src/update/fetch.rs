//! Getting things over HTTPS. Strata uses the system's own `curl` (in
//! /usr/bin on macOS, in System32 on Windows 10 1803 and later) rather than
//! a Rust TLS stack: it keeps a TLS library and its certificate handling out
//! of the binary, uses the system's certificate store and proxy settings, and
//! gets the system's security fixes. The download's integrity doesn't rest on
//! TLS anyway: every file is checked against an ed25519 signature (`sig`).
//!
//! curl runs with the user's `.curlrc` ignored (`-q`), HTTPS only, for
//! redirects too, a bounded number of redirects, a size cap, a connect
//! timeout and a minimum speed. The `Fetch` trait lets tests swap it out.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Returned when the user cancelled.
pub const CANCELLED: &str = "cancelled";

pub trait Fetch: Send + Sync {
    /// GET `url` into memory, at most `limit` bytes.
    fn get(&self, url: &str, limit: u64, cancel: &AtomicBool) -> Result<Vec<u8>, String>;
    /// Download `url` to `dest` (`size` bytes are expected; more is an
    /// error), calling `progress` with the bytes so far.
    fn download(&self, url: &str, dest: &Path, size: u64, cancel: &AtomicBool, progress: &dyn Fn(u64)) -> Result<(), String>;
}

pub struct Curl;

impl Fetch for Curl {
    fn get(&self, url: &str, limit: u64, cancel: &AtomicBool) -> Result<Vec<u8>, String> {
        let tmp = temp_file("get");
        let r = run(url, &tmp, limit, Some(60), cancel, &|_| {}).and_then(|()| {
            let mut out = Vec::new();
            std::fs::File::open(&tmp).and_then(|f| f.take(limit + 1).read_to_end(&mut out)).map_err(|e| e.to_string())?;
            if out.len() as u64 > limit { Err("the response is too big".to_owned()) } else { Ok(out) }
        });
        let _ = std::fs::remove_file(&tmp);
        r
    }

    fn download(&self, url: &str, dest: &Path, size: u64, cancel: &AtomicBool, progress: &dyn Fn(u64)) -> Result<(), String> {
        let r = run(url, dest, size, None, cancel, progress);
        if r.is_err() {
            let _ = std::fs::remove_file(dest);
        }
        r
    }
}

fn temp_file(tag: &str) -> PathBuf {
    use std::sync::atomic::AtomicU64;
    static N: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!("strata-update-{tag}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)))
}

/// The system's curl.
fn curl_path() -> PathBuf {
    if cfg!(windows) {
        let root = std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
        root.join("System32").join("curl.exe")
    } else {
        PathBuf::from("/usr/bin/curl")
    }
}

/// curl's arguments for fetching `url` into `out`.
pub fn curl_args(url: &str, out: &Path, limit: u64, max_time: Option<u64>) -> Vec<std::ffi::OsString> {
    // -q must come first: don't read ~/.curlrc.
    let mut a: Vec<String> = ["-q", "--silent", "--show-error", "--fail", "--location", "--proto", "=https", "--proto-redir", "=https", "--tlsv1.2", "--max-redirs", "5", "--connect-timeout", "20"].map(String::from).to_vec();
    a.extend(["--max-filesize".into(), limit.to_string()]);
    // Give up on a stalled transfer: under 1 KB/s for a minute.
    a.extend(["--speed-limit", "1024", "--speed-time", "60"].map(String::from));
    if let Some(t) = max_time {
        a.extend(["--max-time".into(), t.to_string()]);
    }
    a.extend(["--user-agent".into(), format!("Strata/{} (+https://github.com/{})", env!("CARGO_PKG_VERSION"), super::manifest::REPO)]);
    // GitHub's API wants this; downloads ignore it.
    a.extend(["--header".into(), "Accept: application/vnd.github+json, application/json, application/octet-stream".into()]);
    let mut a: Vec<std::ffi::OsString> = a.into_iter().map(Into::into).collect();
    a.extend(["--output".into(), out.as_os_str().to_owned(), "--url".into(), url.into()]);
    a
}

fn run(url: &str, out: &Path, limit: u64, max_time: Option<u64>, cancel: &AtomicBool, progress: &dyn Fn(u64)) -> Result<(), String> {
    super::manifest::check_url(url)?;
    let curl = curl_path();
    let mut cmd = Command::new(&curl);
    cmd.args(curl_args(url, out, limit, max_time)).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    no_window(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| {
        if cfg!(windows) {
            format!("couldn't run {} ({e}); Windows 10 version 1803 or later includes it", curl.display())
        } else {
            format!("couldn't run {}: {e}", curl.display())
        }
    })?;
    let mut stderr = child.stderr.take();
    // Read curl's messages as they come so the pipe never fills.
    let errors = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(e) = stderr.as_mut() {
            let _ = e.take(16 * 1024).read_to_string(&mut s);
        }
        s
    });
    let status = loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(CANCELLED.into());
        }
        let so_far = std::fs::metadata(out).map_or(0, |m| m.len());
        if so_far > limit {
            let _ = child.kill();
            let _ = child.wait();
            return Err("the download is bigger than the update feed says".into());
        }
        progress(so_far);
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => return Err(e.to_string()),
        }
    };
    let stderr = errors.join().unwrap_or_default();
    if status.success() {
        progress(std::fs::metadata(out).map_or(0, |m| m.len()));
        return Ok(());
    }
    Err(explain(status.code(), stderr.trim()))
}

/// curl's exit code as something a person can act on.
pub fn explain(code: Option<i32>, stderr: &str) -> String {
    let detail = stderr.lines().last().unwrap_or("").trim_start_matches("curl: ").to_owned();
    let what = match code {
        Some(5 | 6) => "Couldn't find GitHub's server. Are you online?".to_owned(),
        Some(7) => "Couldn't connect to GitHub. Are you online?".to_owned(),
        Some(22) if detail.contains("404") => "GitHub has no update information yet (404).".to_owned(),
        Some(22) => format!("GitHub answered with an error ({detail})."),
        Some(28) => "GitHub took too long to answer.".to_owned(),
        Some(35 | 51 | 53 | 54 | 58 | 59 | 60 | 66 | 77 | 80 | 82 | 83 | 90 | 91) => format!("Couldn't make a secure connection to GitHub ({detail})."),
        Some(1) => "Refused to follow a redirect that wasn't HTTPS.".to_owned(),
        Some(47) => "Too many redirects.".to_owned(),
        Some(63) => "The download is bigger than the update feed says.".to_owned(),
        Some(c) => format!("The download failed (curl exit {c}{}).", if detail.is_empty() { String::new() } else { format!(": {detail}") }),
        None => "The download was stopped.".to_owned(),
    };
    what
}

/// Don't flash a console window for curl or tar (Windows).
pub fn no_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// A pretend internet: URL → bytes; records what was fetched.
    #[derive(Default)]
    pub struct FakeNet {
        pub files: Mutex<HashMap<String, Vec<u8>>>,
        pub fetched: Mutex<Vec<String>>,
        /// Hold downloads until this is false (to test progress and cancel).
        pub stall: AtomicBool,
    }

    impl FakeNet {
        pub fn with(files: &[(&str, Vec<u8>)]) -> Self {
            let n = Self::default();
            n.files.lock().unwrap().extend(files.iter().map(|(u, b)| (u.to_string(), b.clone())));
            n
        }

        fn body(&self, url: &str, limit: u64) -> Result<Vec<u8>, String> {
            super::super::manifest::check_url(url)?;
            self.fetched.lock().unwrap().push(url.to_owned());
            let body = self.files.lock().unwrap().get(url).cloned().ok_or_else(|| explain(Some(22), "curl: (22) The requested URL returned error: 404"))?;
            if body.len() as u64 > limit {
                return Err(explain(Some(63), ""));
            }
            Ok(body)
        }
    }

    impl Fetch for FakeNet {
        fn get(&self, url: &str, limit: u64, _cancel: &AtomicBool) -> Result<Vec<u8>, String> {
            self.body(url, limit)
        }

        fn download(&self, url: &str, dest: &Path, size: u64, cancel: &AtomicBool, progress: &dyn Fn(u64)) -> Result<(), String> {
            let body = self.body(url, size)?;
            progress(body.len() as u64 / 2);
            while self.stall.load(Ordering::Relaxed) {
                if cancel.load(Ordering::Relaxed) {
                    return Err(CANCELLED.into());
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            std::fs::write(dest, &body).map_err(|e| e.to_string())?;
            progress(body.len() as u64);
            Ok(())
        }
    }

    #[test]
    fn curl_runs_locked_down() {
        let args: Vec<String> = curl_args("https://github.com/a", Path::new("/tmp/x"), 1000, Some(30)).into_iter().map(|a| a.into_string().unwrap()).collect();
        assert_eq!(args[0], "-q", "-q only works first");
        let pair = |flag: &str| args.iter().position(|a| a == flag).map(|i| args[i + 1].as_str());
        assert_eq!(pair("--proto"), Some("=https"));
        assert_eq!(pair("--proto-redir"), Some("=https"));
        assert_eq!(pair("--max-filesize"), Some("1000"));
        assert_eq!(pair("--max-time"), Some("30"));
        assert_eq!(pair("--max-redirs"), Some("5"));
        assert_eq!(pair("--url"), Some("https://github.com/a"), "the URL is never taken for an option");
        assert!(args.contains(&"--fail".to_owned()));
    }

    #[test]
    fn refuses_bad_urls_before_running_anything() {
        let no = AtomicBool::new(false);
        for url in ["http://github.com/x", "https://evil.example/x", "file:///etc/passwd"] {
            assert!(Curl.get(url, 10, &no).is_err(), "{url}");
        }
    }

    #[test]
    fn errors_read_well() {
        assert!(explain(Some(6), "").contains("online"));
        assert!(explain(Some(22), "curl: (22) The requested URL returned error: 404").contains("404"));
        assert!(explain(Some(60), "curl: (60) SSL certificate problem").contains("secure"));
        assert!(explain(Some(99), "curl: (99) odd").contains("odd"));
    }
}
