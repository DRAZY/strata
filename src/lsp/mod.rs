//! Language Server Protocol client: completions, diagnostics, hover,
//! go to definition, references, rename and formatting from the
//! language's own server. `hub` runs the servers; `app::lsp_ui` puts
//! what they say on screen.

pub mod config;
pub mod edit;
#[cfg(test)]
pub mod fake;
pub mod hub;
pub mod jsonrpc;
pub mod position;
pub mod server;
pub mod uri;

pub use hub::{Hub, State};

#[cfg(test)]
mod tests {
    use super::*;
    use ropey::Rope;
    use serde_json::json;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("strata-lsp-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Run `step` until it returns Some, for up to 20 seconds.
    fn until<T>(what: &str, mut step: impl FnMut() -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(v) = step() {
                return v;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn a_server_starts_syncs_answers_and_shuts_down() {
        let dir = scratch("hub");
        let file = dir.join("a.fake");
        std::fs::write(&file, "").unwrap();
        let mut hub = Hub::new(false);
        hub.use_servers(vec![fake::config("fake", &["*.fake"], json!({}))]);
        let sync = |hub: &mut Hub, v: u64, text: &str| hub.sync(1, &file, (1, v), || Rope::from_str(text), None, std::slice::from_ref(&dir));
        sync(&mut hub, 0, "let bad = 1;\n");
        until("the server to be ready", || hub.server_for(1, &file).map(|_| ()));
        let d = until("diagnostics", || hub.diagnostics(1, &file).filter(|d| !d.items.is_empty()));
        assert_eq!(d.items.len(), 1);
        assert_eq!(d.items[0].range, (position::Pos::new(0, 4), position::Pos::new(0, 7)));
        assert_eq!(d.version, Some(1));

        // Incremental changes: the server's copy matches ours (it reports by position).
        sync(&mut hub, 1, "let 👋 = bad; meh\n");
        let d = until("new diagnostics", || hub.diagnostics(1, &file).filter(|d| d.version == Some(2)));
        let ranges: Vec<_> = d.items.iter().map(|i| (i.severity, i.range)).collect();
        assert_eq!(ranges, [(1, (position::Pos::new(0, 9), position::Pos::new(0, 12))), (2, (position::Pos::new(0, 14), position::Pos::new(0, 17)))]);

        // A request, answered off the UI thread.
        let (server, version) = hub.server_for(1, &file).unwrap();
        assert_eq!(version, 2);
        let mut p = server.request("textDocument/hover", json!({"textDocument": {"uri": uri::from_path(&file)}, "position": {"line": 0, "character": 10}}));
        let r = p.wait(Duration::from_secs(10)).unwrap().unwrap();
        assert!(r["contents"]["value"].as_str().unwrap().contains("Hover for `bad`"), "{r}");

        // Another window can't take over the file, and closing it clears it.
        hub.sync(2, &file, (9, 9), || Rope::from_str("other"), None, &[]);
        assert!(hub.server_for(2, &file).is_none());
        let pid = hub.pid_for(&file).unwrap();
        hub.end_frame(1, &[]);
        assert!(hub.server_for(1, &file).is_none());
        drop(hub);
        assert!(!process_alive(pid), "the server exits when Strata quits");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn utf8_columns_when_the_server_agrees() {
        let dir = scratch("utf8");
        let file = dir.join("a.fake");
        let mut hub = Hub::new(false);
        hub.use_servers(vec![fake::config("fake", &["*.fake"], json!({"utf8": true}))]);
        hub.sync(1, &file, (1, 0), || Rope::from_str("日本 bad\n"), None, &[]);
        let (server, _) = until("ready", || hub.server_for(1, &file).map(|(s, v)| (s.caps(), v)));
        assert_eq!(server.encoding, position::Encoding::Utf8);
        let d = until("diagnostics", || hub.diagnostics(1, &file).filter(|d| !d.items.is_empty()));
        assert_eq!(d.items[0].range.0, position::Pos::new(0, 7), "two 3-byte chars and a space");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_crashed_server_restarts_and_gets_its_files_back() {
        let dir = scratch("crash");
        let file = dir.join("a.fake");
        let marker = dir.join("crashed");
        let mut hub = Hub::new(false);
        hub.use_servers(vec![fake::config("fake", &["*.fake"], json!({"marker": marker}))]);
        let roots = [dir.clone()];
        hub.sync(1, &file, (1, 0), || Rope::from_str("fine\n"), None, &roots);
        until("ready", || hub.server_for(1, &file).map(|_| ()));
        let first = hub.pid_for(&file).unwrap();
        hub.sync(1, &file, (1, 1), || Rope::from_str("CRASH_NOW bad\n"), None, &roots);
        until("the crash", || {
            hub.tick();
            matches!(hub.state_for(&file), Some((_, State::Restarting { .. }))).then_some(())
        });
        assert!(marker.exists());
        // After the pause it's back, with the file sent again.
        let d = until("diagnostics from the new process", || {
            hub.tick();
            hub.sync(1, &file, (1, 1), || Rope::from_str("CRASH_NOW bad\n"), None, &roots);
            hub.diagnostics(1, &file).filter(|d| !d.items.is_empty())
        });
        assert_eq!(d.items.len(), 1);
        assert_ne!(hub.pid_for(&file), Some(first));
        assert_eq!(hub.state_for(&file).map(|s| s.1), Some(State::Ready));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn missing_programs_and_files_without_a_server() {
        let mut hub = Hub::new(false);
        let mut missing = fake::config("ghost", &["*.ghost"], json!({}));
        missing.command = "strata-no-such-language-server".into();
        hub.use_servers(vec![missing]);
        hub.sync(1, Path::new("/tmp/x.ghost"), (1, 0), || Rope::from_str(""), None, &[]);
        hub.sync(1, Path::new("/tmp/x.txt"), (1, 0), || Rope::from_str(""), None, &[]);
        assert_eq!(hub.state_for(Path::new("/tmp/x.ghost")), None);
        let notes = hub.take_notes();
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("strata-no-such-language-server"), "{notes:?}");
        hub.sync(1, Path::new("/tmp/y.ghost"), (1, 0), || Rope::from_str(""), None, &[]);
        assert!(hub.take_notes().is_empty(), "said once");
    }

    fn process_alive(pid: u32) -> bool {
        #[cfg(unix)]
        {
            // Signal 0 checks for the process without touching it.
            let r = unsafe { libc::kill(pid as libc::pid_t, 0) };
            r == 0
        }
        #[cfg(not(unix))]
        {
            let _ = pid;
            false
        }
    }
}
