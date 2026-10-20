//! The manager with the fake plugin (`plugins::fake`): consent, every API
//! method, crashes and restarts, floods, plugins that never answer,
//! document events, activation, settings, keys.

use super::*;
use crate::plugins::api::{METHOD_NOT_FOUND, OUTSIDE_WORKSPACE, PERMISSION_DENIED, STOPPED};
use crate::plugins::fake;
use crate::plugins::package::tests::temp;
use std::collections::BTreeSet;

/// Install a fake plugin named `name` in `config` (the manifest's other
/// fields from `extra`).
pub(crate) fn install_fake(config: &Path, name: &str, words: &[&str], perms: &[&str], extra: Value) -> PathBuf {
    let dir = config.join("Plugins").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let mut m = json!({
        "name": name,
        "version": "1.0.0",
        "api": API,
        "run": {"command": fake::COMMAND, "args": words},
        "permissions": perms,
        "contributes": {"commands": [
            {"id": "read", "title": "Read"}, {"id": "upper", "title": "Upper"}, {"id": "notify", "title": "Notify"},
            {"id": "crash", "title": "Crash"}, {"id": "hang", "title": "Hang"}, {"id": "status", "title": "Status"},
            {"id": "pick", "title": "Pick"}, {"id": "panel", "title": "Panel"}, {"id": "stale", "title": "Stale"}
        ]}
    });
    if let Value::Object(extra) = extra {
        for (k, v) in extra {
            m[k] = v;
        }
    }
    std::fs::write(dir.join(crate::plugins::manifest::FILE), m.to_string()).unwrap();
    dir
}

/// Tick until `f` says something, for up to 20 seconds.
pub(crate) fn until<T>(m: &mut Manager, what: &str, mut f: impl FnMut(&mut Manager) -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        m.tick();
        if let Some(v) = f(m) {
            return v;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn state(m: &Manager, name: &str) -> String {
    m.plugins[m.index(name).unwrap()].state.label()
}

fn running(m: &Manager, name: &str) -> bool {
    matches!(m.plugins[m.index(name).unwrap()].state, State::Running)
}

/// Start `name`, say yes to the question, wait until it runs.
fn start_granted(m: &mut Manager, name: &str) {
    m.start(name);
    until(m, "the consent question", |m| m.question().map(|_| ()));
    m.answer(true);
    until(m, "the plugin to run", |m| running(m, name).then_some(()));
}

/// Ask the running fake plugin to run `cmd` with `args`; answer window
/// requests with `window` meanwhile. The plugin's answer.
fn command(m: &mut Manager, name: &str, cmd: &str, args: Value, mut window: impl FnMut(&mut Manager, String, u64, Incoming)) -> Reply {
    let mut p = m.request(name, "command/execute", json!({"command": cmd, "args": args})).expect("running");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        m.tick();
        for (plugin, run, inc) in m.take_requests(1, 16) {
            m.handled(&plugin, run, inc.size);
            window(m, plugin, run, inc);
        }
        if let Some(r) = p.poll() {
            return r;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {cmd}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// A plugin call through the fake plugin's `call` command: Strata's answer.
fn call(m: &mut Manager, name: &str, method: &str, params: Value) -> Value {
    let r = command(m, name, "call", json!({"method": method, "params": params}), |_, _, _, _| panic!("no window request expected"));
    r.expect("the fake plugin answers")
}

fn alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // SAFETY: signal 0 only checks whether the process exists.
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

fn wait_gone(pid: u32) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while alive(pid) {
        assert!(Instant::now() < deadline, "process {pid} still running");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn consent_is_asked_before_the_first_start_and_tied_to_the_package() {
    let config = temp("mgr-consent");
    let dir = install_fake(&config, "fake", &[], &["notify", "read_document"], json!({}));
    let mut m = Manager::new(Some(&config), None);
    // A command starts it, but only after the question.
    m.run_command("fake", "notify", None);
    let (p, sha, manifest) = until(&mut m, "the question", |m| m.question().map(|(p, s, mf)| (p.name.clone(), s.to_owned(), mf.permissions.clone())));
    assert_eq!(p, "fake");
    assert_eq!(sha, package::hash_dir(&dir).unwrap());
    assert_eq!(manifest, BTreeSet::from([Perm::Notify, Perm::ReadDocument]));
    assert!(m.plugins[0].pid().is_none(), "nothing runs before the answer");
    // No: nothing runs, and a file opening doesn't ask again this session.
    m.answer(false);
    assert!(m.take_notes().iter().any(|n| n.contains("wasn't started")));
    for _ in 0..20 {
        m.tick();
    }
    assert!(m.question().is_none() && m.plugins[0].pid().is_none());
    // Yes: it runs, and the queued command reaches it once it's up.
    start_granted(&mut m, "fake");
    let granted = Store::load(Some(&config));
    assert!(granted.granted("fake", &sha, &manifest), "saved");
    drop(m);

    // Another run of Strata: the same package starts without asking.
    let mut m = Manager::new(Some(&config), None);
    m.start("fake");
    until(&mut m, "the plugin to run", |m| running(m, "fake").then_some(()));
    assert!(m.question().is_none());
    m.stop("fake");

    // Any change to the package asks again: a file added…
    std::fs::write(dir.join("extra.py"), "print('new')").unwrap();
    m.start("fake");
    until(&mut m, "the question after a change", |m| m.question().map(|_| ()));
    m.answer(true);
    until(&mut m, "the plugin to run", |m| running(m, "fake").then_some(()));
    m.stop("fake");
    // …or the manifest asking for more.
    let text = std::fs::read_to_string(dir.join("strata-plugin.json")).unwrap().replace("\"read_document\"", "\"read_document\",\"network\"");
    std::fs::write(dir.join("strata-plugin.json"), text).unwrap();
    m.start("fake");
    let perms = until(&mut m, "the question after a manifest change", |m| m.question().map(|(_, _, mf)| mf.permissions.clone()));
    assert!(perms.contains(&Perm::Network), "the question shows the new permission");
    m.answer(false);
    drop(m);
    std::fs::remove_dir_all(config).ok();
}

#[test]
fn host_methods_and_permission_refusals() {
    let base = std::fs::canonicalize(temp("mgr-host")).unwrap();
    let config = base.join("config");
    let project = base.join("project");
    std::fs::create_dir_all(project.join("src")).unwrap();
    std::fs::write(project.join("src/a.txt"), "inside").unwrap();
    std::fs::create_dir_all(base.join("secret")).unwrap();
    std::fs::write(base.join("secret/key"), "TOP SECRET").unwrap();
    install_fake(&config, "full", &[], &["read_workspace", "write_workspace", "notify"], json!({}));
    install_fake(&config, "none", &[], &[], json!({}));
    let mut m = Manager::new(Some(&config), None);
    m.sync_window(1, Vec::new(), vec![project.clone()], true);
    start_granted(&mut m, "full");
    start_granted(&mut m, "none");

    // log: no permission needed, kept in the plugin's own log.
    assert_eq!(call(&mut m, "none", "log", json!({"message": "hello log", "level": "warning"})), json!({"result": null}));
    assert!(log_lines(&m.plugins[m.index("none").unwrap()].log).iter().any(|l| l == "warning: hello log"));
    // workspace/folders, readFile, writeFile.
    assert_eq!(call(&mut m, "full", "workspace/folders", json!({})), json!({"result": {"folders": [project]}}));
    assert_eq!(call(&mut m, "full", "workspace/readFile", json!({"path": project.join("src/a.txt")})), json!({"result": {"text": "inside"}}));
    assert_eq!(call(&mut m, "full", "workspace/writeFile", json!({"path": project.join("src/b.txt"), "text": "written"})), json!({"result": {"written": true}}));
    assert_eq!(std::fs::read_to_string(project.join("src/b.txt")).unwrap(), "written");
    // Outside the folders, and through a link: refused.
    let r = call(&mut m, "full", "workspace/writeFile", json!({"path": base.join("secret/key"), "text": "pwned"}));
    assert_eq!(r["error"]["code"], OUTSIDE_WORKSPACE);
    let r = call(&mut m, "full", "workspace/readFile", json!({"path": project.join("../secret/key")}));
    assert_eq!(r["error"]["code"], OUTSIDE_WORKSPACE);
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(base.join("secret"), project.join("link")).unwrap();
        let r = call(&mut m, "full", "workspace/writeFile", json!({"path": project.join("link/key"), "text": "pwned"}));
        assert_eq!(r["error"]["code"], OUTSIDE_WORKSPACE, "{r}");
        let r = call(&mut m, "full", "workspace/readFile", json!({"path": project.join("link/key")}));
        assert_eq!(r["error"]["code"], OUTSIDE_WORKSPACE, "{r}");
    }
    assert_eq!(std::fs::read_to_string(base.join("secret/key")).unwrap(), "TOP SECRET");
    // Bad parameters.
    let r = call(&mut m, "full", "workspace/readFile", json!({"path": 42}));
    assert_eq!(r["error"]["code"], api::INVALID_PARAMS);
    // An unknown method.
    let r = call(&mut m, "full", "workspace/rmrf", json!({}));
    assert_eq!(r["error"]["code"], METHOD_NOT_FOUND);

    // Every gated method, without its permission: refused before anything happens, and logged.
    for method in ["document/read", "document/applyEdits", "workspace/folders", "workspace/readFile", "workspace/writeFile", "ui/showPanel", "ui/notify", "ui/statusItem", "ui/pick"] {
        let params = json!({"path": project.join("src/c.txt"), "text": "x", "id": "s", "message": "m", "items": ["a"], "edits": [], "version": 1, "document": 1});
        let r = call(&mut m, "none", method, params);
        assert_eq!(r["error"]["code"], PERMISSION_DENIED, "{method}: {r}");
        assert!(log_lines(&m.plugins[m.index("none").unwrap()].log).iter().any(|l| l.starts_with(&format!("Refused {method}"))), "{method} logged");
    }
    assert!(!project.join("src/c.txt").exists());
    // A refused notification is dropped (and logged) too: nothing reaches a window.
    command(&mut m, "none", "call", json!({"method": "ui/notify", "params": {"message": "sneaky"}, "notify": true}), |_, _, _, _| {}).unwrap();
    until(&mut m, "the refusal in the log", |m| log_lines(&m.plugins[m.index("none").unwrap()].log).iter().filter(|l| l.starts_with("Refused ui/notify")).count().checked_sub(2).map(|_| ()));
    assert!(m.take_requests(1, 100).is_empty());
    drop(m);
    std::fs::remove_dir_all(base).ok();
}

#[test]
fn window_methods_reach_the_window_that_has_the_document() {
    let config = temp("mgr-window");
    install_fake(&config, "fake", &[], &["read_document", "edit_document", "panel", "notify"], json!({}));
    let mut m = Manager::new(Some(&config), None);
    let doc = |id: u64| DocInfo { id, path: None, name: format!("doc{id}"), version: 1, syntax: "Plain Text".into() };
    m.sync_window(1, vec![doc(10)], Vec::new(), true);
    m.sync_window(2, vec![doc(20)], Vec::new(), false);
    start_granted(&mut m, "fake");
    // A request about window 2's document goes to window 2 only.
    let mut p = m.request("fake", "command/execute", json!({"command": "call", "args": {"method": "document/read", "params": {"document": 20}}})).unwrap();
    let got = until(&mut m, "the request", |m| {
        assert!(m.take_requests(1, 10).is_empty(), "window 1 doesn't get window 2's document");
        let r = m.take_requests(2, 10);
        (!r.is_empty()).then_some(r)
    });
    let (name, run, inc) = got.into_iter().next().unwrap();
    assert_eq!((name.as_str(), inc.method.as_str()), ("fake", "document/read"));
    m.reply(&name, run, inc.id, Ok(json!({"text": "from window 2"})));
    let r = loop {
        m.tick();
        if let Some(r) = p.poll() {
            break r;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(r.unwrap(), json!({"result": {"text": "from window 2"}}));
    // Requests about no document go to the focused window.
    m.sync_window(1, vec![doc(10)], Vec::new(), true);
    let r = command(&mut m, "fake", "notify", json!({}), |m, name, run, inc| {
        assert_eq!(inc.method, "ui/notify");
        assert_eq!(inc.params["message"], "Hello from the fake plugin");
        m.reply(&name, run, inc.id, Ok(Value::Null));
    });
    assert_eq!(r.unwrap(), json!({"result": null}));
    drop(m);
    std::fs::remove_dir_all(config).ok();
}

#[test]
fn a_crashing_plugin_is_restarted_then_given_up_on() {
    let config = temp("mgr-crash");
    install_fake(&config, "fake", &[], &["notify"], json!({}));
    let mut m = Manager::new(Some(&config), None);
    start_granted(&mut m, "fake");
    let mut pids = vec![m.plugins[0].pid().unwrap()];
    for round in 1..=MAX_CRASHES {
        m.run_command("fake", "crash", None);
        until(&mut m, "the crash", |m| (!running(m, "fake")).then_some(()));
        wait_gone(*pids.last().unwrap());
        if round < MAX_CRASHES {
            assert!(state(&m, "fake").contains("restarting in"), "{}", state(&m, "fake"));
            let started = Instant::now();
            until(&mut m, "the restart", |m| running(m, "fake").then_some(()));
            // The pause doubles: 1 s, then 2 s.
            assert!(started.elapsed() >= Duration::from_millis(900 * (1 << (round - 1))), "waited {:?}", started.elapsed());
            let pid = m.plugins[0].pid().unwrap();
            assert!(!pids.contains(&pid), "a new process");
            pids.push(pid);
        }
    }
    let s = state(&m, "fake");
    assert!(s.contains("won't restart it"), "{s}");
    assert!(m.take_notes().iter().any(|n| n.contains("keeps stopping")));
    for _ in 0..50 {
        m.tick();
    }
    assert!(m.plugins[0].pid().is_none(), "not restarted");
    // Starting it by hand works again.
    m.start("fake");
    until(&mut m, "the manual start", |m| running(m, "fake").then_some(()));
    drop(m);
    std::fs::remove_dir_all(config).ok();
}

#[test]
fn a_flooding_plugin_is_held_back_without_stalling_the_window() {
    let config = temp("mgr-flood");
    install_fake(&config, "fake", &[], &["notify"], json!({}));
    let mut m = Manager::new(Some(&config), None);
    m.sync_window(1, Vec::new(), Vec::new(), true);
    start_granted(&mut m, "fake");
    let mut flood = m.request("fake", "command/execute", json!({"command": "flood", "args": {"count": 50000}})).unwrap();
    let started = Instant::now();
    let (mut frames, mut notes, mut slowest) = (0, 0, Duration::ZERO);
    let answer = loop {
        let t = Instant::now();
        m.tick();
        let got = m.take_requests(1, 32);
        assert!(got.len() <= 32, "bounded per frame");
        assert!(m.inbox_len("fake") <= INBOX);
        notes += got.len();
        for (name, run, inc) in got {
            m.handled(&name, run, inc.size);
            m.reply(&name, run, inc.id, Ok(Value::Null));
        }
        slowest = slowest.max(t.elapsed());
        frames += 1;
        if let Some(r) = flood.poll() {
            break r;
        }
        assert!(started.elapsed() < Duration::from_secs(60), "the flood never finished");
        std::thread::sleep(Duration::from_millis(2));
    };
    assert_eq!(answer.unwrap(), json!("flooded"));
    // The notifications sent after the answer still arrive.
    until(&mut m, "every notification", |m| {
        notes += m.take_requests(1, 32).len();
        (notes == 200).then_some(())
    });
    assert!(slowest < Duration::from_millis(200), "a frame took {slowest:?}");
    let log = &m.plugins[0].log;
    let (len, dropped) = {
        let l = host::lock(log);
        (l.len(), l.dropped)
    };
    assert_eq!(len, host::LOG_LINES, "the log is bounded");
    assert!(dropped >= 49_000, "older lines were dropped ({dropped})");
    // And it still answers.
    let r = command(&mut m, "fake", "events", json!({}), |_, _, _, _| {});
    assert!(r.is_ok());
    eprintln!("flood: {frames} frames, slowest {slowest:?}");
    drop(m);
    std::fs::remove_dir_all(config).ok();
}

#[test]
fn plugins_that_never_answer_time_out() {
    let config = temp("mgr-silent");
    install_fake(&config, "silent", &["silent"], &[], json!({}));
    install_fake(&config, "hangs", &[], &[], json!({}));
    let mut m = Manager::new(Some(&config), None);
    m.init_timeout = Duration::from_secs(1);
    m.command_timeout = Duration::from_secs(1);
    m.start("silent");
    until(&mut m, "the question", |m| m.question().map(|_| ()));
    m.answer(true);
    let pid = until(&mut m, "the process", |m| m.plugins[m.index("silent").unwrap()].pid());
    let s = until(&mut m, "giving up", |m| matches!(m.plugins[m.index("silent").unwrap()].state, State::Failed(_)).then(|| state(m, "silent")));
    assert!(s.contains("didn't answer within 1 s"), "{s}");
    wait_gone(pid);
    // A command that never finishes: Strata says so and the plugin keeps running.
    start_granted(&mut m, "hangs");
    m.run_command("hangs", "hang", None);
    let note = until(&mut m, "the timeout", |m| m.take_notes().into_iter().find(|n| n.contains("didn't finish in time")));
    assert!(note.contains("Hang"), "{note}");
    assert!(running(&m, "hangs"));
    drop(m);
    std::fs::remove_dir_all(config).ok();
}

#[test]
fn plugins_that_fail_to_start_say_why() {
    let config = temp("mgr-fail");
    install_fake(&config, "refuses", &["refuse"], &[], json!({}));
    install_fake(&config, "exits", &["exit-at-start"], &[], json!({}));
    install_fake(&config, "badapi", &["bad-api"], &[], json!({}));
    std::fs::create_dir_all(config.join("Plugins/missing")).unwrap();
    std::fs::write(config.join("Plugins/missing/strata-plugin.json"), json!({"name": "missing", "version": "1", "api": API, "run": {"command": "strata-no-such-program-anywhere"}, "contributes": {"commands": [{"id": "a", "title": "A"}]}}).to_string()).unwrap();
    let mut m = Manager::new(Some(&config), None);
    for (name, want) in [("refuses", "refused to start: no thanks"), ("exits", "stopped before it started"), ("badapi", "speaks strata.plugin/9"), ("missing", "couldn't find strata-no-such-program-anywhere")] {
        m.start(name);
        until(&mut m, "the question", |m| m.question().map(|_| ()));
        m.answer(true);
        let s = until(&mut m, "the failure", |m| matches!(m.plugins[m.index(name).unwrap()].state, State::Failed(_)).then(|| state(m, name)));
        assert!(s.contains(want), "{name}: {s}");
        assert!(m.plugins[m.index(name).unwrap()].pid().is_none());
    }
    drop(m);
    std::fs::remove_dir_all(config).ok();
}

#[cfg(unix)]
#[test]
fn stopping_or_crashing_ends_everything_the_plugin_started() {
    let config = temp("mgr-tree");
    install_fake(&config, "fake", &[], &[], json!({}));
    let mut m = Manager::new(Some(&config), None);
    start_granted(&mut m, "fake");
    // Stopped by Strata.
    let child = command(&mut m, "fake", "spawn-child", json!({}), |_, _, _, _| {}).unwrap().as_u64().unwrap() as u32;
    assert!(alive(child));
    m.stop("fake");
    wait_gone(child);
    // Exiting on its own.
    m.start("fake");
    until(&mut m, "the plugin to run", |m| running(m, "fake").then_some(()));
    let child = command(&mut m, "fake", "spawn-child", json!({}), |_, _, _, _| {}).unwrap().as_u64().unwrap() as u32;
    m.run_command("fake", "crash", None);
    until(&mut m, "the crash", |m| (!running(m, "fake")).then_some(()));
    wait_gone(child);
    drop(m);
    std::fs::remove_dir_all(config).ok();
}

#[test]
fn documents_are_reported_only_with_read_permission() {
    let config = temp("mgr-docs");
    install_fake(&config, "reader", &[], &["read_document"], json!({}));
    install_fake(&config, "blind", &[], &[], json!({}));
    let mut m = Manager::new(Some(&config), None);
    let doc = |v: u64| DocInfo { id: 7, path: Some(PathBuf::from("/tmp/notes.md")), name: "notes.md".into(), version: v, syntax: "Markdown".into() };
    m.sync_window(1, vec![doc(1)], Vec::new(), true);
    start_granted(&mut m, "reader");
    start_granted(&mut m, "blind");
    let events = |m: &mut Manager, name: &str| -> Vec<String> {
        let r = command(m, name, "events", json!({}), |_, _, _, _| {}).unwrap();
        r.as_array().unwrap().iter().map(|e| e["method"].as_str().unwrap().to_owned()).collect()
    };
    until(&mut m, "opened", |m| events(m, "reader").contains(&"document/opened".to_owned()).then_some(()));
    m.sync_window(1, vec![doc(2)], Vec::new(), true);
    until(&mut m, "changed", |m| events(m, "reader").contains(&"document/changed".to_owned()).then_some(()));
    m.saved(&doc(2));
    m.sync_window(1, Vec::new(), Vec::new(), true);
    let got = until(&mut m, "closed", |m| {
        let e = events(m, "reader");
        e.contains(&"document/closed".to_owned()).then_some(e)
    });
    assert_eq!(got.iter().filter(|e| e.starts_with("document/")).cloned().collect::<Vec<_>>(), ["document/opened", "document/changed", "document/saved", "document/closed"]);
    // The text itself is never in an event (a plugin reads it with document/read).
    let r = command(&mut m, "reader", "events", json!({}), |_, _, _, _| {}).unwrap();
    assert!(r.as_array().unwrap().iter().all(|e| e["params"]["document"].get("text").is_none()));
    assert!(events(&mut m, "blind").iter().all(|e| !e.starts_with("document/")), "nothing without read_document");
    drop(m);
    std::fs::remove_dir_all(config).ok();
}

#[test]
fn opening_or_saving_a_matching_file_starts_a_plugin() {
    let config = temp("mgr-activate");
    install_fake(&config, "md", &[], &[], json!({"activation": ["open:*.md"]}));
    install_fake(&config, "rs", &[], &[], json!({"activation": ["save:*.rs"]}));
    let mut m = Manager::new(Some(&config), None);
    let doc = |id: u64, name: &str| DocInfo { id, path: Some(PathBuf::from(format!("/tmp/{name}"))), name: name.into(), version: 1, syntax: String::new() };
    m.sync_window(1, vec![doc(1, "a.txt")], Vec::new(), true);
    for _ in 0..10 {
        m.tick();
    }
    assert!(m.question().is_none(), "a .txt starts neither");
    m.sync_window(1, vec![doc(1, "a.txt"), doc(2, "b.md")], Vec::new(), true);
    let asked = until(&mut m, "the question for md", |m| m.question().map(|(p, _, _)| p.name.clone()));
    assert_eq!(asked, "md");
    m.answer(true);
    until(&mut m, "md to run", |m| running(m, "md").then_some(()));
    m.saved(&doc(3, "main.rs"));
    let asked = until(&mut m, "the question for rs", |m| m.question().map(|(p, _, _)| p.name.clone()));
    assert_eq!(asked, "rs");
    m.answer(true);
    until(&mut m, "rs to run", |m| running(m, "rs").then_some(()));
    // A turned-off plugin doesn't start.
    m.set_enabled("md", false);
    assert!(!running(&m, "md"));
    m.sync_window(1, vec![doc(4, "c.md")], Vec::new(), true);
    for _ in 0..20 {
        m.tick();
    }
    assert!(!running(&m, "md") && m.question().is_none());
    drop(m);
    std::fs::remove_dir_all(config).ok();
}

#[test]
fn startup_plugins_start_with_strata() {
    let config = temp("mgr-startup");
    install_fake(&config, "early", &[], &[], json!({"activation": ["startup"]}));
    let mut m = Manager::new(Some(&config), None);
    until(&mut m, "the question", |m| m.question().map(|_| ()));
    m.answer(true);
    until(&mut m, "the plugin to run", |m| running(m, "early").then_some(()));
    drop(m);
    std::fs::remove_dir_all(config).ok();
}

#[test]
fn settings_come_from_defaults_and_settings_json() {
    let config = temp("mgr-settings");
    let settings = json!({"contributes": {"commands": [{"id": "read", "title": "Read"}], "settings": {"limit": {"default": 10}, "mode": {"default": "fast"}}}});
    install_fake(&config, "fake", &[], &[], settings);
    let mut m = Manager::new(Some(&config), None);
    let user = |v: Value| -> BTreeMap<String, Map<String, Value>> { serde_json::from_value(json!({"fake": v})).unwrap() };
    m.set_user_settings(&user(json!({"limit": 99, "mode": 3, "unknown": true})));
    start_granted(&mut m, "fake");
    let init = command(&mut m, "fake", "init", json!({}), |_, _, _, _| {}).unwrap();
    assert_eq!(init["settings"], json!({"limit": 99, "mode": "fast"}), "a wrong type keeps the default");
    assert_eq!(init["api"], API);
    assert_eq!(init["permissions"], json!([]));
    assert!(init["workspaceFolders"].is_null(), "folders only with a workspace permission");
    assert!(init["storagePath"].as_str().unwrap().ends_with("PluginData/fake") || init["storagePath"].as_str().unwrap().ends_with("PluginData\\fake"));
    let log = log_lines(&m.plugins[0].log);
    assert!(log.iter().any(|l| l.contains("mode") && l.contains("wrong type")));
    assert!(log.iter().any(|l| l.contains("unknown") && l.contains("isn't one of")));
    m.set_user_settings(&user(json!({"limit": 5})));
    let s = until(&mut m, "settings/changed", |m| {
        let s = command(m, "fake", "settings", json!({}), |_, _, _, _| {}).unwrap();
        (s["limit"] == 5).then_some(s)
    });
    assert_eq!(s, json!({"limit": 5, "mode": "fast"}));
    drop(m);
    std::fs::remove_dir_all(config).ok();
}

#[test]
fn status_items_are_limited_and_checked() {
    let config = temp("mgr-status");
    install_fake(&config, "fake", &[], &["notify"], json!({}));
    let mut m = Manager::new(Some(&config), None);
    start_granted(&mut m, "fake");
    for k in 0..STATUS_ITEMS {
        m.set_status_item("fake", &json!({"id": format!("i{k}"), "text": format!("item {k}\nsecond line")})).unwrap();
    }
    assert_eq!(m.status_items().len(), STATUS_ITEMS);
    assert_eq!(m.status_items()[0].text, "item 0", "one line");
    assert_eq!(m.set_status_item("fake", &json!({"id": "more", "text": "x"})).unwrap_err().code, BUSY);
    m.set_status_item("fake", &json!({"id": "i0", "text": ""})).unwrap();
    assert_eq!(m.status_items().len(), STATUS_ITEMS - 1);
    let long = "w".repeat(500);
    m.set_status_item("fake", &json!({"id": "i1", "text": long, "command": "read"})).unwrap();
    assert_eq!(m.status_items().iter().find(|s| s.id == "i1").unwrap().text.chars().count(), STATUS_TEXT);
    assert!(m.set_status_item("fake", &json!({"id": "i1", "text": "x", "command": "not-ours"})).is_err());
    // A crash takes its items away.
    m.run_command("fake", "crash", None);
    until(&mut m, "the crash", |m| (!running(m, "fake")).then_some(()));
    assert!(m.status_items().is_empty());
    drop(m);
    std::fs::remove_dir_all(config).ok();
}

#[test]
fn plugin_keys_never_take_strata_keys_and_conflicts_are_reported() {
    let config = temp("mgr-keys");
    let keys = |k: &str| json!({"contributes": {"commands": [{"id": "read", "title": "Read"}], "keybindings": [{"command": "read", "key": k}]}});
    install_fake(&config, "a-first", &[], &[], keys("primary+alt+9"));
    install_fake(&config, "b-second", &[], &[], keys("primary+alt+9"));
    install_fake(&config, "c-third", &[], &[], keys("primary+p"));
    let m = Manager::new(Some(&config), None);
    let keymap = crate::keybindings::Keymap::defaults();
    let (bound, conflicts) = m.key_bindings(|mods, key| keymap.is_bound(mods, key));
    assert_eq!(bound.len(), 1);
    assert_eq!((bound[0].plugin.as_str(), bound[0].key), ("a-first", egui::Key::Num9));
    assert_eq!(conflicts.len(), 2);
    assert!(conflicts.iter().any(|c| c.plugin == "b-second" && c.why.contains("a-first")));
    assert!(conflicts.iter().any(|c| c.plugin == "c-third" && c.why.contains("Strata already uses")), "{conflicts:?}");
    drop(m);
    std::fs::remove_dir_all(config).ok();
}

#[test]
fn commands_name_the_document_only_to_plugins_that_may_read_or_edit_it() {
    let config = temp("mgr-cmd-doc");
    let cmds = json!({"contributes": {"commands": [{"id": "read", "title": "Read"}]}});
    install_fake(&config, "reader", &[], &["read_document"], cmds.clone());
    install_fake(&config, "editor", &[], &["edit_document"], cmds.clone());
    install_fake(&config, "blind", &[], &["notify", "panel"], cmds);
    let mut m = Manager::new(Some(&config), None);
    let doc = DocInfo { id: 3, path: Some(PathBuf::from("/secret/plans.md")), name: "plans.md".into(), version: 4, syntax: "Markdown".into() };
    for (name, sees) in [("reader", true), ("editor", true), ("blind", false)] {
        start_granted(&mut m, name);
        // Strata's own command/execute, then (behind it, in order) the question.
        m.run_command(name, "read", Some(doc.clone()));
        let docs = command(&mut m, name, "docs", json!({}), |m, n, r, inc| m.reply(&n, r, inc.id, Err(RpcError::new(api::NO_DOCUMENT, "test")))).unwrap();
        let first = &docs[0];
        if sees {
            assert_eq!(first["path"], "/secret/plans.md", "{name}: {docs}");
            assert_eq!(first["version"], 4);
        } else {
            assert!(first.is_null(), "{name} isn't told which file is in front: {docs}");
        }
    }
    drop(m);
    std::fs::remove_dir_all(config).ok();
}

#[test]
fn oversized_requests_are_refused_before_reaching_a_window() {
    let config = temp("mgr-big");
    install_fake(&config, "fake", &[], &["panel", "notify"], json!({}));
    let mut m = Manager::new(Some(&config), None);
    m.sync_window(1, Vec::new(), Vec::new(), true);
    start_granted(&mut m, "fake");
    let big = "x".repeat(3 << 20);
    let r = call(&mut m, "fake", "ui/showPanel", json!({"title": "t", "markdown": big}));
    assert_eq!(r["error"]["code"], api::TOO_LARGE, "{}", r["error"]);
    let r = call(&mut m, "fake", "ui/notify", json!({"message": "y".repeat(300 << 10)}));
    assert_eq!(r["error"]["code"], api::TOO_LARGE);
    assert!(log_lines(&m.plugins[0].log).iter().any(|l| l == "Refused ui/showPanel: it's too large"));
    // A JSON size estimate close to the real one.
    let v = json!({"a": "héllo", "list": [1, true, null, "x"]});
    let real = v.to_string().len();
    let est = host::json_size(&v);
    assert!(est >= real / 2 && est <= real * 2, "{est} vs {real}");
    drop(m);
    std::fs::remove_dir_all(config).ok();
}

#[test]
fn big_answers_are_never_dropped_however_many_are_asked_for() {
    let config = temp("mgr-burst");
    install_fake(&config, "fake", &[], &["read_document"], json!({}));
    let mut m = Manager::new(Some(&config), None);
    m.sync_window(1, Vec::new(), Vec::new(), true);
    start_granted(&mut m, "fake");
    // Twelve reads at once, each answered with 10 MB: far over what Strata
    // keeps queued for a plugin, so it has to wait between them.
    let text = "w".repeat(10 << 20);
    let mut answered = 0;
    let r = command(&mut m, "fake", "burst", json!({"method": "document/read", "params": {}, "count": 12}), |m, name, run, inc| {
        answered += 1;
        m.reply(&name, run, inc.id, Ok(json!({"text": text})));
    });
    assert_eq!(r.unwrap(), json!({"results": 12, "errors": 0}));
    assert_eq!(answered, 12);
    assert!(!log_lines(&m.plugins[0].log).iter().any(|l| l.contains("dropped")), "{:?}", log_lines(&m.plugins[0].log));
    drop(m);
    std::fs::remove_dir_all(config).ok();
}

#[test]
fn the_consent_line_moves_on_when_a_waiting_plugin_goes_away() {
    let config = temp("mgr-line");
    install_fake(&config, "a-first", &[], &[], json!({}));
    install_fake(&config, "b-second", &[], &[], json!({}));
    let mut m = Manager::new(Some(&config), None);
    m.start("a-first");
    until(&mut m, "the first question", |m| m.question().map(|_| ()));
    m.start("b-second");
    until(&mut m, "the second to wait", |m| matches!(m.plugins[1].state, State::Asking { .. }).then_some(()));
    // The first plugin disappears from disk while its question is up.
    std::fs::remove_dir_all(config.join("Plugins/a-first")).unwrap();
    m.reload();
    let (name, _, _) = m.question().expect("the next question");
    assert_eq!(name.name, "b-second");
    m.answer(true);
    until(&mut m, "the second to run", |m| matches!(m.plugins[0].state, State::Running).then_some(()));
    drop(m);
    std::fs::remove_dir_all(config).ok();
}

#[test]
fn answers_to_an_earlier_run_never_reach_a_new_one() {
    let config = temp("mgr-runs");
    install_fake(&config, "fake", &[], &["notify"], json!({}));
    let mut m = Manager::new(Some(&config), None);
    m.sync_window(1, Vec::new(), Vec::new(), true);
    start_granted(&mut m, "fake");
    let mut p = m.request("fake", "command/execute", json!({"command": "notify"})).unwrap();
    let (name, run, inc) = until(&mut m, "the request", |m| m.take_requests(1, 1).into_iter().next());
    // It crashes and restarts before the window answers.
    m.stop("fake");
    assert_eq!(p.wait(Duration::from_secs(5)).unwrap().unwrap_err().code, STOPPED);
    m.start("fake");
    until(&mut m, "the restart", |m| running(m, "fake").then_some(()));
    m.reply(&name, run, inc.id, Ok(json!("stale")));
    assert_eq!(m.plugins[0].run, run + 1);
    let r = command(&mut m, "fake", "events", json!({}), |_, _, _, _| {});
    assert!(r.is_ok(), "the new run is unaffected");
    drop(m);
    std::fs::remove_dir_all(config).ok();
}

/// A plugin whose program is a script in its package (`run.sh`), which
/// starts the fake plugin with `words`; `PLUGIN_TEST_EXE` comes from the
/// test environment (`parent_env_for_tests`).
#[cfg(unix)]
fn install_script(config: &Path, name: &str, words: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let dir = config.join("Plugins").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let m = json!({"name": name, "version": "1.0.0", "api": API, "run": {"command": "./run.sh"}, "contributes": {"commands": [{"id": "read", "title": "Read"}]}});
    std::fs::write(dir.join(crate::plugins::manifest::FILE), m.to_string()).unwrap();
    write_script(&dir.join("run.sh"), words);
    let _ = std::fs::Permissions::from_mode(0o700);
    dir
}

#[cfg(unix)]
fn write_script(path: &Path, words: &str) {
    use std::os::unix::fs::PermissionsExt;
    let body = format!("#!/bin/sh\nSTRATA_FAKE_PLUGIN='{words}' exec \"$PLUGIN_TEST_EXE\" --exact plugins::fake::fake_plugin --nocapture --test-threads=1 -q\n");
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}

#[cfg(unix)]
fn script_manager(config: &Path) -> Manager {
    let mut m = Manager::new(Some(config), None);
    let exe = std::env::current_exe().unwrap();
    m.parent_env_for_tests = vec![("PLUGIN_TEST_EXE".into(), exe.into_os_string())];
    m
}

#[cfg(unix)]
#[test]
fn the_approved_snapshot_runs_not_files_changed_since() {
    let config = temp("mgr-snapshot");
    let dir = install_script(&config, "script", "");
    let mut m = script_manager(&config);
    m.start("script");
    let sha = until(&mut m, "the question", |m| m.question().map(|(_, s, _)| s.to_owned()));
    // While the question is up, the installed copy is changed to one that can't start.
    write_script(&dir.join("run.sh"), "exit-at-start");
    m.answer(true);
    until(&mut m, "the approved code to run", |m| match &m.plugins[0].state {
        State::Running => Some(()),
        State::Failed(why) => panic!("the changed files ran: {why}"),
        _ => None,
    });
    let snap = config.join("Plugins/.snapshots").join(&sha);
    assert!(snap.join("package/run.sh").is_file(), "it runs from its snapshot");
    assert!(snap.join(".strata-snapshot").is_file(), "marked as Strata's");
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(snap.join("package/run.sh")).unwrap().permissions().mode() & 0o222, 0, "read-only");
    }
    m.stop("script");
    // The changed install is another package: it asks again.
    m.start("script");
    let again = until(&mut m, "a new question", |m| m.question().map(|(_, s, _)| s.to_owned()));
    assert_ne!(again, sha);
    m.answer(false);
    drop(m);
    // Old snapshots nothing was granted for go when Strata starts.
    age(&snap);
    age(&config.join("Plugins/.snapshots").join(&again));
    let m = script_manager(&config);
    assert!(snap.exists(), "the granted one stays");
    assert!(!config.join("Plugins/.snapshots").join(&again).exists(), "the refused one is pruned");
    drop(m);
    crate::plugins::package::unseal_for_tests(&config.join("Plugins"));
    std::fs::remove_dir_all(config).ok();
}

#[cfg(unix)]
#[test]
fn a_tampered_snapshot_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    let config = temp("mgr-tamper");
    install_script(&config, "script", "");
    let mut m = script_manager(&config);
    m.start("script");
    let sha = until(&mut m, "the question", |m| m.question().map(|(_, s, _)| s.to_owned()));
    // Changed while the question is up: refused right before it would run.
    let snap = config.join("Plugins/.snapshots").join(&sha);
    std::fs::set_permissions(&snap, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::set_permissions(snap.join("package"), std::fs::Permissions::from_mode(0o700)).unwrap();
    write_script(&snap.join("package/run.sh"), "refuse");
    m.answer(true);
    let why = until(&mut m, "the refusal", |m| if let State::Failed(w) = &m.plugins[0].state { Some(w.clone()) } else { None });
    assert!(why.contains("changed"), "{why}");
    assert!(m.plugins[0].pid().is_none());
    // Started again: a fresh copy of the (unchanged) install runs.
    m.start("script");
    until(&mut m, "the plugin to run", |m| matches!(m.plugins[0].state, State::Running).then_some(()));
    drop(m);
    crate::plugins::package::unseal_for_tests(&config.join("Plugins"));
    std::fs::remove_dir_all(config).ok();
}

#[test]
fn plugins_get_a_clean_environment() {
    if !std::process::Command::new("python3").arg("--version").output().is_ok_and(|o| o.status.success()) {
        eprintln!("python3 isn't installed: skipped");
        return;
    }
    let config = temp("mgr-env");
    let evil = config.join("evil");
    std::fs::create_dir_all(&evil).unwrap();
    std::fs::write(evil.join("strata_probe.py"), "FOUND = True\n").unwrap();
    let dir = config.join("Plugins/probe");
    std::fs::create_dir_all(&dir).unwrap();
    let m = json!({"name": "probe", "version": "1.0.0", "api": API, "run": {"command": "python3", "args": ["probe.py"]}, "contributes": {"commands": [{"id": "env", "title": "Env"}]}});
    std::fs::write(dir.join(crate::plugins::manifest::FILE), m.to_string()).unwrap();
    // A minimal plugin that says whether it could import a module from PYTHONPATH.
    let probe = r#"import json, os, sys
def read():
    n = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            sys.exit(0)
        line = line.strip()
        if not line:
            if n is not None:
                return json.loads(sys.stdin.buffer.read(n))
            continue
        if line.lower().startswith(b"content-length"):
            n = int(line.split(b":")[1])
def send(m):
    b = json.dumps(m).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(b) + b)
    sys.stdout.buffer.flush()
try:
    import strata_probe
    found = True
except ImportError:
    found = False
while True:
    m = read()
    if m.get("method") == "initialize":
        send({"jsonrpc": "2.0", "id": m["id"], "result": {"api": "strata.plugin/1"}})
    elif m.get("method") == "command/execute":
        keys = ["PYTHONPATH", "PYTHONSTARTUP", "NODE_OPTIONS", "DYLD_INSERT_LIBRARIES", "LD_PRELOAD", "PYTHONNOUSERSITE", "PYTHONSAFEPATH"]
        send({"jsonrpc": "2.0", "id": m["id"], "result": {"found": found, "env": {k: os.environ.get(k) for k in keys}}})
    elif "id" in m:
        send({"jsonrpc": "2.0", "id": m["id"], "result": None})
"#;
    std::fs::write(dir.join("probe.py"), probe).unwrap();
    let mut m = Manager::new(Some(&config), None);
    let e = |s: &str| std::ffi::OsString::from(s);
    m.parent_env_for_tests = vec![
        (e("PYTHONPATH"), evil.clone().into_os_string()),
        (e("PYTHONSTARTUP"), evil.join("strata_probe.py").into_os_string()),
        (e("NODE_OPTIONS"), e("--require /tmp/evil.js")),
        (e("DYLD_INSERT_LIBRARIES"), e("/tmp/evil.dylib")),
        (e("LD_PRELOAD"), e("/tmp/evil.so")),
    ];
    start_granted(&mut m, "probe");
    let r = command(&mut m, "probe", "env", json!({}), |_, _, _, _| {}).unwrap();
    assert_eq!(r["found"], false, "PYTHONPATH wasn't honored: {r}");
    for k in ["PYTHONPATH", "PYTHONSTARTUP", "NODE_OPTIONS", "DYLD_INSERT_LIBRARIES", "LD_PRELOAD"] {
        assert!(r["env"][k].is_null(), "{k} reached the plugin: {r}");
    }
    assert_eq!((r["env"]["PYTHONNOUSERSITE"].as_str(), r["env"]["PYTHONSAFEPATH"].as_str()), (Some("1"), Some("1")));
    drop(m);
    crate::plugins::package::unseal_for_tests(&config.join("Plugins"));
    std::fs::remove_dir_all(config).ok();
}

/// The snapshot folder of `sha` (its marker, lock and `package` copy).
#[cfg(unix)]
fn snap_of(config: &Path, sha: &str) -> PathBuf {
    config.join("Plugins/.snapshots").join(sha)
}

/// Make a snapshot look a day and more old (prune leaves younger ones).
#[cfg(unix)]
fn age(dir: &Path) {
    let old = std::time::SystemTime::now() - Duration::from_secs(3 * 86_400);
    std::fs::File::open(dir).unwrap().set_modified(old).unwrap();
}

#[cfg(unix)]
#[test]
fn snapshots_live_only_in_a_real_private_folder() {
    use std::os::unix::fs::PermissionsExt;
    let config = temp("mgr-snaproot");
    install_script(&config, "script", "");
    // `.snapshots` is a link to somewhere else, holding a folder prune would like.
    let outside = config.join("outside");
    let bait = outside.join("a".repeat(64));
    std::fs::create_dir_all(&bait).unwrap();
    std::fs::write(bait.join(".strata-snapshot"), "strata plugin snapshot\n").unwrap();
    std::fs::write(bait.join("precious"), "keep me").unwrap();
    age(&bait);
    std::os::unix::fs::symlink(&outside, config.join("Plugins/.snapshots")).unwrap();
    let mut m = script_manager(&config);
    assert!(bait.join("precious").exists(), "prune didn't follow the link");
    m.start("script");
    let why = until(&mut m, "the refusal", |m| if let State::Failed(w) = &m.plugins[0].state { Some(w.clone()) } else { None });
    assert!(why.contains("isn't a plain folder"), "{why}");
    drop(m);
    assert!(bait.join("precious").exists());
    // A real folder others may write to is refused too.
    std::fs::remove_file(config.join("Plugins/.snapshots")).unwrap();
    std::fs::create_dir(config.join("Plugins/.snapshots")).unwrap();
    std::fs::set_permissions(config.join("Plugins/.snapshots"), std::fs::Permissions::from_mode(0o770)).unwrap();
    let mut m = script_manager(&config);
    m.start("script");
    let why = until(&mut m, "the refusal", |m| if let State::Failed(w) = &m.plugins[0].state { Some(w.clone()) } else { None });
    assert!(why.contains("other users"), "{why}");
    drop(m);
    std::fs::remove_dir_all(config).ok();
}

#[cfg(unix)]
#[test]
fn prune_removes_only_old_unused_marked_snapshot_folders() {
    let config = temp("mgr-prune");
    install_script(&config, "script", "");
    let mut m = script_manager(&config);
    start_granted(&mut m, "script");
    drop(m);
    let root = config.join("Plugins/.snapshots");
    let mk = |name: &str, marker: bool| {
        let d = root.join(name);
        std::fs::create_dir_all(&d).unwrap();
        if marker {
            std::fs::write(d.join(".strata-snapshot"), "strata plugin snapshot\n").unwrap();
        }
        std::fs::write(d.join("data"), "x").unwrap();
        age(&d);
        d
    };
    let unmarked = mk(&"b".repeat(64), false);
    let odd_name = mk("not-a-hash", true);
    let young = root.join("c".repeat(64));
    std::fs::create_dir_all(&young).unwrap();
    std::fs::write(young.join(".strata-snapshot"), "strata plugin snapshot\n").unwrap();
    let old_unused = mk(&"d".repeat(64), true);
    let granted: Vec<PathBuf> = std::fs::read_dir(&root).unwrap().flatten().map(|e| e.path()).filter(|p| p.join("package").exists()).collect();
    assert_eq!(granted.len(), 1);
    age(&granted[0]);
    let m = script_manager(&config);
    drop(m);
    assert!(!old_unused.exists(), "old, unused, marked: removed");
    for keep in [&unmarked, &odd_name, &young, &granted[0]] {
        assert!(keep.exists(), "{} was kept", keep.display());
    }
    std::fs::remove_dir_all(config).ok();
}

#[cfg(unix)]
#[test]
fn prune_skips_a_snapshot_another_strata_is_running() {
    use std::io::BufRead;
    let config = temp("mgr-prune-lock");
    install_script(&config, "script", "");
    let mut m = script_manager(&config);
    m.start("script");
    let sha = until(&mut m, "the question", |m| m.question().map(|(_, s, _)| s.to_owned()));
    m.answer(false);
    drop(m);
    let snap = snap_of(&config, &sha);
    age(&snap);
    // Another process (the test binary as a helper) runs that snapshot: it holds its lock.
    let mut helper = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "plugins::package::tests::lock_helper", "--nocapture", "--test-threads=1", "-q"])
        .env("STRATA_TEST_LOCK_HELPER", snap.join(".strata-lock"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut out = std::io::BufReader::new(helper.stdout.take().unwrap());
    let mut line = String::new();
    while line.trim() != "locked" {
        line.clear();
        assert!(out.read_line(&mut line).unwrap() > 0, "the helper didn't lock");
    }
    let m = script_manager(&config);
    drop(m);
    assert!(snap.exists(), "in use elsewhere: kept");
    drop(helper.stdin.take());
    helper.wait().unwrap();
    let m = script_manager(&config);
    drop(m);
    assert!(!snap.exists(), "no longer in use, old and refused: removed");
    std::fs::remove_dir_all(config).ok();
}

#[cfg(unix)]
#[test]
fn a_snapshot_that_cant_be_made_read_only_doesnt_run() {
    use std::os::unix::fs::PermissionsExt;
    let config = temp("mgr-seal");
    install_script(&config, "script", "");
    crate::plugins::package::fail_seal_for_tests(&config);
    let mut m = script_manager(&config);
    m.start("script");
    let why = until(&mut m, "the refusal", |m| if let State::Failed(w) = &m.plugins[0].state { Some(w.clone()) } else { None });
    assert!(why.contains("read-only"), "{why}");
    assert!(m.plugins[0].pid().is_none());
    drop(m);
    // Sealed, then one file made writable again (same bytes): refused before it runs.
    let config2 = temp("mgr-seal2");
    install_script(&config2, "script", "");
    let mut m = script_manager(&config2);
    start_granted(&mut m, "script");
    m.stop("script");
    let sha = std::fs::read_dir(config2.join("Plugins/.snapshots")).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).find(|n| n.len() == 64).unwrap();
    let script = snap_of(&config2, &sha).join("package/run.sh");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    m.start("script");
    let why = until(&mut m, "the refusal", |m| if let State::Failed(w) = &m.plugins[0].state { Some(w.clone()) } else { None });
    assert!(why.contains("changed"), "{why}");
    drop(m);
    for c in [config, config2] {
        crate::plugins::package::unseal_for_tests(&c);
        std::fs::remove_dir_all(c).ok();
    }
}
