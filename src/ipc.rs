//! Single instance: a second `strata` hands its paths to the running one.
//!
//! The running Strata (the one holding the session lock) listens on a Unix
//! socket in the config folder (`strata.sock`); on Windows, on a localhost TCP
//! port written with a random token to `strata.port`. A request is one JSON
//! line; the server answers `{"ok":true,...}` at once and, for `--wait`, a
//! second line `closed` when the files' tabs have all been closed.
//!
//! Hardening: a request is at most `MAX_REQUEST` bytes and must arrive within
//! `REQUEST_TIMEOUT`; each connection is handled on its own short-lived
//! thread, so a slow client can't hold up the next one; on Unix both ends
//! check that the other runs as the same user; the Windows token is compared
//! in constant time.

use crate::cli::Target;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

/// Largest request accepted (thousands of paths fit).
const MAX_REQUEST: u64 = 64 * 1024;
/// A whole request must arrive within this.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// Connections handled at once; more are dropped (a real `strata` is
/// answered in milliseconds).
const MAX_HANDLERS: usize = 16;

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
    // socket in a private folder of the runtime (or temp) folder, named after
    // the config folder.
    if path.as_os_str().len() < 100 {
        return path;
    }
    let hash = dir.as_os_str().as_encoded_bytes().iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3));
    fallback_dir().join(format!("{hash:016x}.sock"))
}

/// `$XDG_RUNTIME_DIR/strata-<uid>` (Linux; that folder is the user's alone),
/// else `<temp>/strata-<uid>`. The server creates it owner-only and refuses
/// one someone else made (`private_socket_dir`).
fn fallback_dir() -> PathBuf {
    #[cfg(unix)]
    // SAFETY: getuid has no preconditions and can't fail.
    let uid = unsafe { libc::getuid() };
    #[cfg(not(unix))]
    let uid = 0;
    let base = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(std::env::temp_dir);
    base.join(format!("strata-{uid}"))
}

/// Make sure the socket's folder is ours alone: create it 0700, and refuse a
/// folder that's a symlink or someone else's.
#[cfg(unix)]
fn private_socket_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    crate::private::create_dir_all(dir)?;
    let meta = std::fs::symlink_metadata(dir)?;
    // SAFETY: getuid has no preconditions and can't fail.
    if !meta.is_dir() || meta.uid() != unsafe { libc::getuid() } {
        return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, format!("{} isn't a folder of ours", dir.display())));
    }
    if meta.permissions().mode() & 0o077 != 0 {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// The user id at the other end of a Unix socket.
#[cfg(unix)]
fn peer_uid(s: &Stream) -> std::io::Result<u32> {
    use std::os::fd::AsRawFd;
    let fd = s.as_raw_fd();
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        // SAFETY: `cred` and `len` are valid for writes of the sizes given.
        let r = unsafe { libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_PEERCRED, (&raw mut cred).cast(), &mut len) };
        if r != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(cred.uid)
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        let (mut uid, mut gid) = (0, 0);
        // SAFETY: both out-pointers are valid for writes.
        if unsafe { libc::getpeereid(fd, &mut uid, &mut gid) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(uid)
    }
}

/// The other end runs as this user.
#[cfg(unix)]
fn same_user(s: &Stream) -> bool {
    // SAFETY: getuid has no preconditions and can't fail.
    peer_uid(s).is_ok_and(|uid| uid == unsafe { libc::getuid() })
}

/// On Windows the token in the per-user `strata.port` file stands in for this.
#[cfg(windows)]
fn same_user(_: &Stream) -> bool {
    true
}

/// Compare without stopping at the first difference (no timing hints).
fn same_token(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Reads from a stream until a deadline, however slowly the bytes trickle in.
struct Deadline<'a> {
    stream: &'a Stream,
    end: Instant,
}

impl Read for Deadline<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let left = self.end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "request took too long"));
        }
        self.stream.set_read_timeout(Some(left))?;
        let mut s = self.stream;
        s.read(buf)
    }
}

/// Read one request line: at most `MAX_REQUEST` bytes, within `REQUEST_TIMEOUT`.
fn read_request(conn: &Stream, timeout: Duration) -> std::io::Result<String> {
    let mut line = String::new();
    BufReader::new(Deadline { stream: conn, end: Instant::now() + timeout }.take(MAX_REQUEST)).read_line(&mut line)?;
    if !line.ends_with('\n') {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("request cut off after {} bytes (the limit is {MAX_REQUEST})", line.len())));
    }
    Ok(line)
}

/// One connection: check who it is, read the request, answer, pass it on.
fn handle(mut conn: Stream, token: &str, tx: &mpsc::Sender<Incoming>, wake: &dyn Fn()) {
    if !same_user(&conn) {
        log::warn!("ipc: ignored a connection from another user");
        return;
    }
    let line = match read_request(&conn, REQUEST_TIMEOUT) {
        Ok(l) => l,
        Err(e) => {
            log::warn!("ipc: couldn't read a request: {e}");
            return;
        }
    };
    let req = match serde_json::from_str::<Request>(&line) {
        Ok(r) => r,
        Err(e) => {
            log::warn!("ipc: ignored a malformed request: {e}");
            return;
        }
    };
    if !same_token(&req.token, token) {
        log::warn!("ipc: ignored a request with the wrong token");
        return;
    }
    log::debug!("ipc: open {} path(s){}", req.targets.len(), if req.wait { ", waiting" } else { "" });
    let ack = serde_json::to_string(&Ack { ok: true }).unwrap_or_default() + "\n";
    if let Err(e) = conn.write_all(ack.as_bytes()) {
        log::warn!("ipc: couldn't answer: {e}");
        return;
    }
    let _ = conn.set_read_timeout(None);
    let waiter = req.wait.then_some(Waiter(conn));
    if tx.send(Incoming { targets: req.targets, new_window: req.new_window, waiter }).is_ok() {
        wake();
    }
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
    pub fn start(dir: &Path, wake: impl Fn() + Send + Sync + 'static) -> std::io::Result<Self> {
        crate::private::create_dir_all(dir)?;
        let path = socket_path(dir);
        #[cfg(unix)]
        if let Some(parent) = path.parent().filter(|p| *p != dir) {
            private_socket_dir(parent)?;
        }
        let (listener, token) = listen(&path)?;
        let (tx, rx) = mpsc::channel();
        let token: Arc<str> = token.into();
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(wake);
        let busy = Arc::new(AtomicUsize::new(0));
        std::thread::Builder::new().name("strata-ipc".into()).spawn(move || {
            for conn in listener.incoming() {
                let conn = match conn {
                    Ok(c) => c,
                    Err(e) => {
                        log::warn!("ipc: accept failed: {e}");
                        continue;
                    }
                };
                if busy.fetch_add(1, Ordering::SeqCst) >= MAX_HANDLERS {
                    busy.fetch_sub(1, Ordering::SeqCst);
                    log::warn!("ipc: too many connections at once; dropped one");
                    continue;
                }
                let (token, tx, wake, done) = (token.clone(), tx.clone(), wake.clone(), busy.clone());
                let spawned = std::thread::Builder::new().name("strata-ipc-conn".into()).spawn(move || {
                    handle(conn, &token, &tx, &*wake);
                    done.fetch_sub(1, Ordering::SeqCst);
                });
                if let Err(e) = spawned {
                    busy.fetch_sub(1, Ordering::SeqCst);
                    log::warn!("ipc: couldn't start a handler: {e}");
                }
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
    crate::private::write(path, format!("{}\n{token}\n", l.local_addr()?.port()))?;
    Ok((l, token))
}

/// 256 bits for the token. `RandomState` keys come from the OS random source
/// (seeded once per thread, then stepped), so each hash is keyed by a secret
/// 128-bit value; no extra crate needed (`getrandom` isn't in the Windows build).
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
    let s = Stream::connect(path).ok()?;
    if !same_user(&s) {
        log::warn!("ipc: {} belongs to another user; not sending to it", path.display());
        return None;
    }
    Some((s, String::new()))
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
    fn tokens_compare_whole() {
        assert!(same_token("abc123", "abc123"));
        assert!(same_token("", ""));
        assert!(!same_token("abc123", "abc124"));
        assert!(!same_token("abc", "abc123"));
    }

    /// A connected pair of streams.
    #[cfg(unix)]
    fn pair() -> (Stream, Stream) {
        Stream::pair().unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn requests_are_capped_and_timed() {
        // Oversized: cut off at MAX_REQUEST, refused.
        let (mut a, b) = pair();
        let writer = std::thread::spawn(move || {
            let big = vec![b'x'; MAX_REQUEST as usize + 10];
            let _ = a.write_all(&big);
        });
        let err = read_request(&b, Duration::from_secs(5)).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData, "{err}");
        drop(b);
        writer.join().unwrap();
        // Trickled bytes don't extend the deadline.
        let (mut a, b) = pair();
        let writer = std::thread::spawn(move || {
            for _ in 0..20 {
                if a.write_all(b"x").is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(30));
            }
        });
        let t = Instant::now();
        assert!(read_request(&b, Duration::from_millis(200)).is_err());
        assert!(t.elapsed() < Duration::from_millis(500), "gave up at the deadline: {:?}", t.elapsed());
        drop(b);
        writer.join().unwrap();
        // A normal line is read.
        let (mut a, b) = pair();
        a.write_all(b"{\"targets\":[]}\n").unwrap();
        assert_eq!(read_request(&b, Duration::from_secs(1)).unwrap(), "{\"targets\":[]}\n");
    }

    #[cfg(unix)]
    #[test]
    fn peers_are_checked_for_the_same_user() {
        let (a, _b) = pair();
        assert_eq!(peer_uid(&a).unwrap(), unsafe { libc::getuid() });
        assert!(same_user(&a));
    }

    #[cfg(unix)]
    #[test]
    fn a_slow_client_does_not_block_the_next() {
        let d = dir("slow");
        let server = Server::start(&d, || {}).unwrap();
        // Connects and says nothing.
        let _idle = Stream::connect(socket_path(&d)).unwrap();
        let t = Instant::now();
        assert!(send(&d, &[Target::plain("/b".into())], false, false).unwrap().is_some());
        assert!(t.elapsed() < Duration::from_secs(2), "answered while another client sat idle");
        assert_eq!(wait_for(&server)[0].targets[0].path, PathBuf::from("/b"));
        drop(server);
        std::fs::remove_dir_all(d).ok();
    }

    #[cfg(unix)]
    #[test]
    fn long_path_sockets_live_in_a_private_folder() {
        use std::os::unix::fs::PermissionsExt;
        let d = dir("deep2").join("a-folder-name-long-enough-to-push-the-socket-path-past-the-unix-limit-of-104-bytes");
        let sock = socket_path(&d);
        let parent = sock.parent().unwrap();
        assert!(parent.file_name().unwrap().to_string_lossy().starts_with("strata-"), "{}", sock.display());
        assert_ne!(parent, std::env::temp_dir(), "not loose in the shared temp folder");
        let server = Server::start(&d, || {}).unwrap();
        assert_eq!(std::fs::metadata(parent).unwrap().permissions().mode() & 0o777, 0o700);
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
