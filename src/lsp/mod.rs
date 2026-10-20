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

pub use hub::{Hub, State, Trust};

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
        let mut p = server.request("textDocument/hover", json!({"textDocument": {"uri": uri::from_path(&hub.server_path(&file))}, "position": {"line": 0, "character": 10}}));
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
    fn a_server_that_never_starts_is_not_retried() {
        let dir = scratch("nostart");
        let file = dir.join("a.fake");
        for builtin in [false, true] {
            let mut hub = Hub::new(false);
            let mut c = fake::config("fake", &["*.fake"], json!({"fail_to_start": true}));
            c.builtin = builtin;
            hub.use_servers(vec![c]);
            hub.sync(1, &file, (1, 0), || Rope::from_str(""), None, &[]);
            let notes = until("the failure", || {
                hub.tick();
                let n = hub.take_notes();
                (!n.is_empty()).then_some(n)
            });
            assert!(notes[0].contains("couldn't start") && notes[0].contains("isn't really here"), "{notes:?}");
            std::thread::sleep(Duration::from_millis(1200));
            hub.tick();
            assert_eq!(hub.pid_for(&file), None, "no new process");
            match hub.state_for(&file) {
                // A server the user set up says it failed; a built-in one stays quiet.
                Some((_, State::Failed(why))) => assert!(!builtin && why.contains("isn't really here")),
                None => assert!(builtin),
                other => panic!("{other:?}"),
            }
        }
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_server_refusing_to_initialize_leaves_no_process() {
        let dir = scratch("refuse");
        let file = dir.join("a.fake");
        let mut hub = Hub::new(false);
        hub.use_servers(vec![fake::config("fake", &["*.fake"], json!({"refuse_init": true}))]);
        hub.sync(1, &file, (1, 0), || Rope::from_str(""), None, &[]);
        let pid = hub.pid_for(&file).expect("started");
        until("the refusal", || {
            hub.tick();
            let n = hub.take_notes();
            (!n.is_empty()).then_some(())
        });
        until("the process to be gone", || (!process_alive(pid)).then_some(()));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn servers_start_only_in_trusted_folders() {
        let dir = scratch("trust");
        let config = scratch("trust-config");
        let file = dir.join("a.fake");
        let sync = |hub: &mut Hub| hub.sync(1, &file, (1, 0), || Rope::from_str("bad"), None, std::slice::from_ref(&dir));
        let mut hub = Hub::new(false);
        hub.use_servers(vec![fake::config("fake", &["*.fake"], json!({}))]);
        hub.require_trust();
        hub.set_config_dir(&config);
        sync(&mut hub);
        assert_eq!(hub.pid_for(&file), None, "nothing runs before the user says");
        let (root, names) = hub.trust_question(1).expect("a question");
        assert_eq!(root, dir);
        assert_eq!(names.len(), 1);
        assert_eq!(hub.trust_question(2), None, "asked in the window that opened the file");
        assert!(matches!(hub.state_for(&file), Some((_, State::NotTrusted))));
        // Not Now: no server, no more asking (this session), until asked again.
        hub.answer_trust(&root, Trust::NotNow);
        sync(&mut hub);
        assert_eq!((hub.pid_for(&file), hub.trust_question(1)), (None, None));
        hub.ask_again(&file);
        sync(&mut hub);
        assert!(hub.trust_question(1).is_some());
        // Always: it starts, and the folder is remembered in Strata's config folder.
        hub.answer_trust(&root, Trust::Always);
        sync(&mut hub);
        assert!(hub.pid_for(&file).is_some());
        let saved: Vec<PathBuf> = serde_json::from_str(&std::fs::read_to_string(config.join(hub::TRUST_FILE)).unwrap()).unwrap();
        assert_eq!(saved, [std::fs::canonicalize(&dir).unwrap()], "stored as the real path");
        drop(hub);
        // Next time (another Strata): no question.
        let mut hub = Hub::new(false);
        hub.use_servers(vec![fake::config("fake", &["*.fake"], json!({}))]);
        hub.require_trust();
        hub.set_config_dir(&config);
        sync(&mut hub);
        assert!(hub.pid_for(&file).is_some() && hub.trust_question(1).is_none());
        // `lsp_trust_all` skips the question everywhere.
        let other = scratch("trust-other");
        let mut hub = Hub::new(false);
        hub.use_servers(vec![fake::config("fake", &["*.fake"], json!({}))]);
        hub.require_trust();
        hub.configure(true, &Default::default(), true);
        hub.sync(1, &other.join("x.fake"), (1, 0), || Rope::from_str(""), None, &[]);
        assert!(hub.trust_question(1).is_none());
        for d in [dir, config, other] {
            std::fs::remove_dir_all(d).ok();
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_inside_a_trusted_folder_doesnt_make_its_target_trusted() {
        let base = scratch("trust-link");
        let (safe, repo) = (base.join("safe"), base.join("repo"));
        std::fs::create_dir_all(&safe).unwrap();
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(repo.join("Cargo.toml"), "").unwrap();
        std::os::unix::fs::symlink(&repo, safe.join("link")).unwrap();
        let config = scratch("trust-link-config");
        let mut hub = Hub::new(false);
        let mut c = fake::config("fake", &["*.fake"], json!({}));
        c.root_markers = vec!["Cargo.toml".into()];
        hub.use_servers(vec![c]);
        hub.require_trust();
        hub.set_config_dir(&config);
        hub.answer_trust(&safe, Trust::Always);
        // The file through the link: its project is really `repo`, not under `safe`.
        let file = safe.join("link").join("main.fake");
        hub.sync(1, &file, (1, 0), || Rope::from_str(""), None, &[]);
        assert_eq!(hub.pid_for(&file), None, "no server for the untrusted repo");
        assert!(hub.trust_question(1).is_some(), "asked instead");
        // Remembered folders are stored as real paths.
        let saved: Vec<PathBuf> = serde_json::from_str(&std::fs::read_to_string(config.join(hub::TRUST_FILE)).unwrap()).unwrap();
        assert_eq!(saved, [std::fs::canonicalize(&safe).unwrap()]);
        // A file really inside `safe` is trusted, even named through a link to it.
        std::fs::create_dir_all(safe.join("own")).unwrap();
        std::os::unix::fs::symlink(&safe, base.join("alias")).unwrap();
        let own = base.join("alias").join("own").join("x.fake");
        hub.sync(1, &own, (1, 0), || Rope::from_str(""), None, &[]);
        assert!(hub.pid_for(&own).is_some());
        std::fs::remove_dir_all(base).ok();
        std::fs::remove_dir_all(config).ok();
    }

    #[test]
    fn trust_fails_closed_when_a_folder_has_no_real_path() {
        let base = std::fs::canonicalize(scratch("trust-closed")).unwrap();
        let mut hub = Hub::new(false);
        hub.use_servers(vec![fake::config("fake", &["*.fake"], json!({}))]);
        hub.require_trust();
        hub.answer_trust(&base, Trust::Once);
        // A new file in a folder that doesn't exist (yet): its project folder can't be resolved.
        let file = base.join("gone").join("new.fake");
        hub.sync(1, &file, (1, 0), || Rope::from_str(""), None, &[]);
        assert_eq!(hub.pid_for(&file), None, "not trusted without a real path");
        let (root, _) = hub.trust_question(1).expect("asked instead");
        hub.answer_trust(&root, Trust::Once);
        assert!(hub.take_notes().iter().any(|n| n.contains("can't be found")));
        hub.sync(1, &file, (1, 0), || Rope::from_str(""), None, &[]);
        assert_eq!((hub.pid_for(&file), hub.trust_question(1)), (None, None), "still nothing, and no loop of questions");
        std::fs::remove_dir_all(base).ok();
    }

    #[cfg(unix)]
    #[test]
    fn a_server_runs_in_the_real_folder_that_was_trusted() {
        let base = scratch("trust-bind");
        let safe = base.join("safe");
        std::fs::create_dir_all(&safe).unwrap();
        std::os::unix::fs::symlink(&safe, base.join("alias")).unwrap();
        let mut hub = Hub::new(false);
        hub.use_servers(vec![fake::config("fake", &["*.fake"], json!({}))]);
        hub.require_trust();
        hub.answer_trust(&safe, Trust::Once);
        let file = base.join("alias").join("x.fake");
        hub.sync(1, &file, (1, 0), || Rope::from_str(""), None, &[]);
        assert!(hub.pid_for(&file).is_some());
        assert_eq!(hub.root_for(&file), Some(std::fs::canonicalize(&safe).unwrap()), "not the link it was reached through");
        std::fs::remove_dir_all(base).ok();
    }

    /// The fake server's hover at the start of `path`'s word `at`: says how often it was opened.
    fn hover_text(hub: &Hub, window: u64, path: &Path, col: u32) -> Option<String> {
        let (server, _) = hub.server_for(window, path)?;
        let mut p = server.request("textDocument/hover", json!({"textDocument": {"uri": uri::from_path(&hub.server_path(path))}, "position": {"line": 0, "character": col}}));
        let r = p.wait(Duration::from_secs(10))?.ok()?;
        r["contents"]["value"].as_str().map(str::to_owned)
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_file_is_sent_and_matched_as_the_file_it_points_to() {
        let dir = scratch("link-file");
        std::fs::write(dir.join("real.fake"), "x bad\n").unwrap();
        std::os::unix::fs::symlink("real.fake", dir.join("link.fake")).unwrap();
        let (link, real) = (dir.join("link.fake"), std::fs::canonicalize(dir.join("real.fake")).unwrap());
        let mut hub = Hub::new(false);
        hub.use_servers(vec![fake::config("fake", &["*.fake"], json!({}))]);
        hub.sync(1, &link, (1, 0), || Rope::from_str("x bad\n"), None, &[]);
        assert_eq!(hub.server_path(&link), real, "the server hears of the file the link points to");
        // Its answers about real.fake land on the tab named link.fake.
        let d = until("diagnostics", || hub.diagnostics(1, &link).filter(|d| !d.items.is_empty()));
        assert_eq!(d.items.len(), 1);
        assert_eq!(hub.client_path(1, &real), uri::normalize(&link));
        std::fs::remove_dir_all(dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn one_file_reached_two_ways_is_one_document() {
        let base = scratch("two-aliases");
        let real = base.join("d");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("x.fake"), "thing\n").unwrap();
        std::os::unix::fs::symlink(&real, base.join("a1")).unwrap();
        std::os::unix::fs::symlink(&real, base.join("a2")).unwrap();
        let (t1, t2) = (base.join("a1/x.fake"), base.join("a2/x.fake"));
        let mut hub = Hub::new(false);
        hub.use_servers(vec![fake::config("fake", &["*.fake"], json!({}))]);
        hub.sync(1, &t1, (1, 0), || Rope::from_str("thing\n"), None, &[]);
        hub.sync(1, &t2, (2, 0), || Rope::from_str("thing\n"), None, &[]);
        until("ready", || hub.server_for(1, &t1).map(|_| ()));
        assert!(hover_text(&hub, 1, &t1, 1).unwrap().contains("opens=1"), "opened once");
        // The first tab speaks for the file; the second has no server of its own.
        assert!(hub.server_for(1, &t2).is_none());
        // Closing the first hands the document to the second; the server keeps it.
        let before = hub.handoffs();
        hub.end_frame(1, std::slice::from_ref(&t2));
        assert_eq!(hub.handoffs(), before + 1, "answers asked for by the first tab are void");
        assert!(hub.server_for(1, &t2).is_none(), "nothing for the new tab until its text has gone out");
        hub.sync(1, &t2, (2, 0), || Rope::from_str("thing\n"), None, &[]);
        assert!(hover_text(&hub, 1, &t2, 1).unwrap().contains("opens=1"), "not closed and opened again");
        hub.end_frame(1, &[]);
        assert!(hub.server_for(1, &t2).is_none());
        std::fs::remove_dir_all(base).ok();
    }

    #[cfg(unix)]
    #[test]
    fn two_windows_spelling_a_file_differently_share_its_document() {
        let base = scratch("two-windows");
        let real = base.join("d");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("x.fake"), "thing\n").unwrap();
        std::os::unix::fs::symlink(&real, base.join("alias")).unwrap();
        let (t1, t2) = (base.join("alias/x.fake"), real.join("x.fake"));
        let mut hub = Hub::new(false);
        hub.use_servers(vec![fake::config("fake", &["*.fake"], json!({}))]);
        hub.sync(1, &t1, (1, 0), || Rope::from_str("thing\n"), None, &[]);
        hub.sync(2, &t2, (2, 0), || Rope::from_str("thing\n"), None, &[]);
        assert_eq!(hub.pid_for(&t1), hub.pid_for(&t2), "one server");
        until("ready", || hub.server_for(1, &t1).map(|_| ()));
        assert!(hub.server_for(2, &t2).is_none(), "window 1 opened it first");
        assert_eq!(hub.client_path(2, &std::fs::canonicalize(&t2).unwrap()), uri::normalize(&t2), "each window gets its own tab's name");
        assert_eq!(hub.client_path(1, &std::fs::canonicalize(&t2).unwrap()), uri::normalize(&t1));
        hub.forget_window(1);
        hub.sync(2, &t2, (2, 0), || Rope::from_str("thing\n"), None, &[]);
        assert!(hover_text(&hub, 2, &t2, 1).unwrap().contains("opens=1"));
        std::fs::remove_dir_all(base).ok();
    }

    #[cfg(unix)]
    #[test]
    fn hard_links_to_one_file_are_one_document() {
        let dir = scratch("hard-link");
        std::fs::write(dir.join("a.fake"), "thing\n").unwrap();
        std::fs::hard_link(dir.join("a.fake"), dir.join("b.fake")).unwrap();
        let (a, b) = (dir.join("a.fake"), dir.join("b.fake"));
        let mut hub = Hub::new(false);
        hub.use_servers(vec![fake::config("fake", &["*.fake"], json!({}))]);
        hub.sync(1, &a, (1, 0), || Rope::from_str("thing\n"), None, &[]);
        hub.sync(1, &b, (2, 0), || Rope::from_str("thing\n"), None, &[]);
        until("ready", || hub.server_for(1, &a).map(|_| ()));
        assert!(hover_text(&hub, 1, &a, 1).unwrap().contains("opens=1"), "one document for both names");
        assert!(hub.server_for(1, &b).is_none(), "the second name follows the first");
        assert_eq!(hub.server_path(&b), hub.server_path(&a));
        std::fs::remove_dir_all(dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn hard_links_separated_by_a_replace_become_two_documents() {
        for trigger in ["change", "focus"] {
            let dir = scratch(&format!("hard-split-{trigger}"));
            std::fs::write(dir.join("a.fake"), "fine\n").unwrap();
            std::fs::hard_link(dir.join("a.fake"), dir.join("b.fake")).unwrap();
            let (a, b) = (dir.join("a.fake"), dir.join("b.fake"));
            let t0 = std::fs::metadata(&a).unwrap().modified().ok();
            let mut hub = Hub::new(false);
            hub.use_servers(vec![fake::config("fake", &["*.fake"], json!({}))]);
            hub.sync(1, &a, (1, 0), || Rope::from_str("fine\n"), t0, &[]);
            hub.sync(1, &b, (2, 0), || Rope::from_str("fine bad\n"), t0, &[]);
            until("ready", || hub.server_for(1, &a).map(|_| ()));
            assert!(hub.server_for(1, &b).is_none(), "one document while they're one file");
            // Another program replaces a.fake: b.fake keeps the old file.
            save_atomically(&a, "fine\n");
            match trigger {
                // Strata notices a.fake changed on disk: its modification time.
                "change" => {
                    let t1 = std::fs::metadata(&a).unwrap().modified().ok();
                    hub.sync(1, &a, (1, 0), || Rope::from_str("fine\n"), t1, &[]);
                }
                _ => hub.recheck_files(),
            }
            hub.sync(1, &b, (2, 0), || Rope::from_str("fine bad\n"), t0, &[]);
            assert_ne!(hub.server_path(&b), hub.server_path(&a), "{trigger}: two files now");
            let d = until("b's own diagnostics", || hub.diagnostics(1, &b).filter(|d| !d.items.is_empty()));
            assert_eq!(d.items.len(), 1, "{trigger}");
            assert!(hub.server_for(1, &a).is_some(), "{trigger}: a keeps its document");
            std::fs::remove_dir_all(dir).ok();
        }
    }

    #[test]
    fn mapping_server_paths_stats_each_file_once() {
        let dir = scratch("map-cost");
        let mut hub = Hub::new(false);
        hub.use_servers(vec![fake::config("fake", &["*.fake"], json!({}))]);
        for k in 0..10 {
            let p = dir.join(format!("open{k}.fake"));
            std::fs::write(&p, "x\n").unwrap();
            hub.sync(1, &p, (k, 0), || Rope::from_str("x\n"), None, &[]);
        }
        // 50 locations: 20 in open files (by their exact names), 30 in 15 other files.
        let mut paths: Vec<PathBuf> = (0..20).map(|k| hub.server_path(&dir.join(format!("open{}.fake", k % 10)))).collect();
        for k in 0..30 {
            let p = dir.join(format!("other{}.fake", k % 15));
            std::fs::write(&p, "y\n").unwrap();
            paths.push(p);
        }
        hub::reset_identity_stats();
        let mapped = hub.client_paths(1, &paths);
        assert_eq!(mapped.len(), 50);
        assert!(mapped[..20].iter().all(|m| matches!(m, hub::Mapped::Tab { .. })));
        assert!(mapped[20..].iter().all(|m| matches!(m, hub::Mapped::Unopened(_))));
        // The 10 open files once each, the 15 other files once each; the exact names none.
        assert_eq!(hub::identity_stats(), 25);
        std::fs::remove_dir_all(dir).ok();
    }

    /// Replace `path` as an atomic save does (Strata's, for a file with one
    /// name): a new file renamed over it, so a new inode.
    fn save_atomically(path: &Path, text: &str) {
        let tmp = path.with_extension("tmp-save");
        std::fs::write(&tmp, text).unwrap();
        std::fs::rename(&tmp, path).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn identity_is_checked_fresh_after_a_save_replaces_the_file() {
        use std::os::unix::fs::MetadataExt;
        let dir = scratch("identity-fresh");
        let x = dir.join("x.fake");
        std::fs::write(&x, "thing\n").unwrap();
        // Another name keeps the file's first inode alive after the save.
        std::fs::hard_link(&x, dir.join("old-inode.fake")).unwrap();
        let first = std::fs::metadata(&x).unwrap().ino();
        let mut hub = Hub::new(false);
        hub.use_servers(vec![fake::config("fake", &["*.fake"], json!({}))]);
        hub.sync(1, &x, (1, 0), || Rope::from_str("thing\n"), None, &[]);
        save_atomically(&x, "thing\n");
        assert_ne!(std::fs::metadata(&x).unwrap().ino(), first, "the save made a new file");
        // A path to the old inode is a different file now: it mustn't land on the tab.
        let stale = std::fs::canonicalize(dir.join("old-inode.fake")).unwrap();
        assert_eq!(hub.client_path(1, &stale), stale);
        // And the tab's file is still found by identity under another name for it.
        std::fs::hard_link(&x, dir.join("new-name.fake")).unwrap();
        assert_eq!(hub.client_path(1, &dir.join("new-name.fake")), uri::normalize(&x));
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
