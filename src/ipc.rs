//! Single instance: a second `strata` hands its paths to the running one.
//!
//! The running Strata (the one holding the session lock) listens on a Unix
//! socket in the config folder (`strata.sock`); on Windows, on a localhost TCP
//! port written with a random token to `strata.port`. A request is one JSON
//! line; the server answers `{"ok":true,...}` at once and, for `--wait`, a
//! second line `closed` when the files' tabs have all been closed.

use crate::cli::Target;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

#[cfg(unix)]
type Stream = std::os::unix::net::UnixStream;
#[cfg(windows)]
type Stream = std::net::TcpStream;

#[derive(Serialize, Deserialize, Debug, Default)]
struct Request {
    #[serde(default)]
    token: String,
    targets: Vec<Target>,
    #[serde(default)]
    wait: bool,
    /// `strata -n`: open in a new window.
    #[serde(default)]
    new_window: bool,
}

/// The server's first reply.
#[derive(Serialize, Deserialize, Debug, Default)]
pub struct Ack {
    pub ok: bool,
}

pub fn socket_path(dir: &Path) -> PathBuf {
    if cfg!(windows) {
        return dir.join("strata.port");
    }
    let path = dir.join("strata.sock");
    // Unix socket paths are limited to ~104 bytes; a deep config folder gets a
    // socket in the (per-user on macOS) temp folder, named after the folder.
    if path.as_os_str().len() < 100 {
        return path;
    }
    let hash = dir.as_os_str().as_encoded_bytes().iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3));
    std::env::temp_dir().join(format!("strata-{hash:016x}.sock"))
}

// ---- server ----------------------------------------------------------------

/// Paths another `strata` asked to open.
pub struct Incoming {
    pub targets: Vec<Target>,
    pub new_window: bool,
    /// Present for `--wait`: call `done` when the files are closed.
    pub waiter: Option<Waiter>,
}

/// The connection of a `strata --wait` that's blocked until its files close.
pub struct Waiter(Stream);

impl Waiter {
    pub fn done(mut self) {
        let _ = self.0.write_all(b"closed\n");
    }
}

pub struct Server {
    rx: mpsc::Receiver<Incoming>,
    path: PathBuf,
}

impl Server {
    /// Listen for other instances. Only the process holding the session lock
    /// may call this: a socket left by a crashed Strata is replaced.
    /// `wake` runs (on a background thread) after each request arrives.
    pub fn start(dir: &Path, wake: impl Fn() + Send + 'static) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let path = socket_path(dir);
        let (listener, token) = listen(&path)?;
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new().name("strata-ipc".into()).spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut conn) = conn else { continue };
                let _ = conn.set_read_timeout(Some(Duration::from_secs(5)));
                let mut line = String::new();
                if BufReader::new(&mut conn).read_line(&mut line).is_err() {
                    continue;
                }
                let Ok(req) = serde_json::from_str::<Request>(&line) else { continue };
                if req.token != token {
                    continue;
                }
                let ack = serde_json::to_string(&Ack { ok: true }).unwrap_or_default() + "\n";
                if conn.write_all(ack.as_bytes()).is_err() {
                    continue;
                }
                let _ = conn.set_read_timeout(None);
                let waiter = req.wait.then_some(Waiter(conn));
                if tx.send(Incoming { targets: req.targets, new_window: req.new_window, waiter }).is_err() {
                    return;
                }
                wake();
            }
        })?;
        Ok(Self { rx, path })
    }

    pub fn poll(&self) -> Vec<Incoming> {
        self.rx.try_iter().collect()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(unix)]
fn listen(path: &Path) -> std::io::Result<(std::os::unix::net::UnixListener, String)> {
    use std::os::unix::fs::PermissionsExt;
    // We own the session, so anything here is left over from a crash.
    let _ = std::fs::remove_file(path);
    let l = std::os::unix::net::UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok((l, String::new()))
}

#[cfg(windows)]
fn listen(path: &Path) -> std::io::Result<(std::net::TcpListener, String)> {
    let l = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    let token = random_token();
    std::fs::write(path, format!("{}\n{token}\n", l.local_addr()?.port()))?;
    Ok((l, token))
}

#[cfg(windows)]
fn random_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut out = String::new();
    for i in 0..4u64 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(i);
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
        h.write_u32(std::process::id());
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

// ---- client ----------------------------------------------------------------

/// A request the running Strata accepted.
pub struct Sent {
    pub ack: Ack,
    reader: BufReader<Stream>,
}

impl Sent {
    /// Block until the server says the files were closed (or it quits).
    pub fn wait_closed(mut self) {
        let mut line = String::new();
        let _ = self.reader.get_mut().set_read_timeout(None);
        let _ = self.reader.read_line(&mut line);
    }
}

/// Hand `targets` to the running Strata. `Ok(None)` when none is listening
/// (no socket, or one left behind by a crash).
pub fn send(dir: &Path, targets: &[Target], wait: bool, new_window: bool) -> std::io::Result<Option<Sent>> {
    let Some((mut stream, token)) = connect(&socket_path(dir)) else { return Ok(None) };
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let req = Request { token, targets: targets.to_vec(), wait, new_window };
    stream.write_all((serde_json::to_string(&req)? + "\n").as_bytes())?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    // Hung up without a word: nobody is really there. (A socket whose owner
    // died can still accept briefly through a child process that inherited it
    // between fork and exec.)
    if reader.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    let ack: Ack = serde_json::from_str(&line).map_err(|e| std::io::Error::other(format!("bad reply from the running Strata: {e}")))?;
    Ok(Some(Sent { ack, reader }))
}

#[cfg(unix)]
fn connect(path: &Path) -> Option<(Stream, String)> {
    Stream::connect(path).ok().map(|s| (s, String::new()))
}

#[cfg(windows)]
fn connect(path: &Path) -> Option<(Stream, String)> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut lines = text.lines();
    let port: u16 = lines.next()?.trim().parse().ok()?;
    let token = lines.next()?.trim().to_string();
    let s = Stream::connect_timeout(&(std::net::Ipv4Addr::LOCALHOST, port).into(), Duration::from_secs(2)).ok()?;
    Some((s, token))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn dir(name: &str) -> PathBuf {
        // Short: Unix socket paths are limited to ~100 bytes.
        let d = std::env::temp_dir().join(format!("strata-ipc-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn wait_for(server: &Server) -> Vec<Incoming> {
        for _ in 0..1000 {
            let got = server.poll();
            if !got.is_empty() {
                return got;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("nothing arrived");
    }

    #[test]
    fn second_instance_hands_paths_to_the_first() {
        let d = dir("rt");
        assert!(send(&d, &[], false, false).unwrap().is_none(), "nobody is listening yet");
        let wakes = Arc::new(AtomicUsize::new(0));
        let w = wakes.clone();
        let server = Server::start(&d, move || {
            w.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
        let t = vec![Target { path: "/x/a.rs".into(), line: Some(3), col: Some(2) }, Target::plain("/x/dir".into())];
        let sent = send(&d, &t, false, false).unwrap().expect("server is listening");
        assert!(sent.ack.ok);
        let got = wait_for(&server);
        assert_eq!(got[0].targets, t);
        assert!(got[0].waiter.is_none());
        assert!(!got[0].new_window);
        assert_eq!(wakes.load(Ordering::SeqCst), 1);

        // --wait: the client blocks until the server reports the files closed.
        let sent = send(&d, &t[..1], true, false).unwrap().unwrap();
        let client = std::thread::spawn(move || sent.wait_closed());
        let mut got = wait_for(&server);
        std::thread::sleep(Duration::from_millis(50));
        assert!(!client.is_finished());
        got.remove(0).waiter.expect("a waiter").done();
        client.join().unwrap();

        // -n: a new window.
        send(&d, &t[..1], false, true).unwrap().unwrap();
        assert!(wait_for(&server)[0].new_window);

        drop(server);
        assert!(!socket_path(&d).exists());
        assert!(send(&d, &[], false, false).unwrap().is_none());
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn deep_config_folders_still_work() {
        let d = dir("deep").join("a-folder-name-long-enough-to-push-the-socket-path-past-the-unix-limit-of-104-bytes");
        std::fs::create_dir_all(&d).unwrap();
        let server = Server::start(&d, || {}).unwrap();
        assert!(send(&d, &[], false, false).unwrap().is_some());
        assert!(wait_for(&server)[0].targets.is_empty());
        drop(server);
        std::fs::remove_dir_all(d.parent().unwrap()).ok();
    }

    #[test]
    fn stale_socket_is_replaced() {
        let d = dir("stale");
        // A crashed Strata leaves its socket (or port file) behind with nobody listening.
        #[cfg(unix)]
        drop(std::os::unix::net::UnixListener::bind(socket_path(&d)).unwrap());
        #[cfg(windows)]
        std::fs::write(socket_path(&d), "1\nnope\n").unwrap();
        assert!(socket_path(&d).exists());
        assert!(send(&d, &[], false, false).unwrap().is_none(), "a stale socket means nobody is running");
        let server = Server::start(&d, || {}).unwrap();
        assert!(send(&d, &[Target::plain("/a".into())], false, false).unwrap().is_some());
        assert_eq!(wait_for(&server)[0].targets[0].path, PathBuf::from("/a"));
        drop(server);
        std::fs::remove_dir_all(d).ok();
    }
}
