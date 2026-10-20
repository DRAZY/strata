//! The helpers end to end in the app, against a fake server on 127.0.0.1
//! (see `ai::http::tests`) and a fake `claude`: consent comes before any
//! connection, answers stream into the panel in whole frames, cancel works,
//! and keys come from the keychain.

use super::super::ui_tests::frame;
use super::*;
use crate::ai::http::tests::{FakeServer, openai_chunk, openai_ok, send_slowly, sse_headers};
use crate::review::tests::temp_repo;
use crate::session::Settings;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::Duration;

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("strata-ai-ui-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A window with AI helpers on, pointed at `url`, with its own config folder.
fn app_with(url: &str, tag: &str) -> StrataApp {
    let settings = Settings { ai_enabled: true, ai_provider: "openai".into(), ai_base_url: url.into(), ai_model: "test-model".into(), ..Settings::default() };
    let mut app = StrataApp::with_settings(Some(temp_dir(&format!("{tag}-config"))), settings, String::new());
    app.push_doc(crate::document::Document::untitled());
    app
}

/// A file with `text`, open and all selected.
fn select_file(app: &mut StrataApp, dir: &Path, name: &str, text: &str) {
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    app.open_path(path, false);
    let i = app.active;
    app.docs[i].editor.select_all();
}

/// The longest a frame may take. A debug build on a busy CI machine can
/// take over half a second for a frame (a fresh context lays out its fonts
/// in the first), so this is generous; what it must catch is the window
/// waiting on the provider, and these tests make that wait long: the fake
/// servers hold their connections for 20 seconds, and the fake `claude`
/// won't answer until the window has drawn a frame while it runs.
const FRAME_LIMIT: Duration = Duration::from_secs(5);

/// Draw frames until `done` (each frame runs the app as a window would).
fn until(app: &mut StrataApp, ctx: &egui::Context, what: &str, done: impl Fn(&StrataApp) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !done(app) {
        let t = Instant::now();
        frame(app, ctx, vec![]);
        assert!(t.elapsed() < FRAME_LIMIT, "a frame took {:?}: the UI waited on something", t.elapsed());
        assert!(Instant::now() < deadline, "timed out waiting for {what}: {:?}", app.ai.answer());
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn finished(app: &StrataApp) -> bool {
    !app.ai.busy()
}

fn last_body(srv: &FakeServer) -> serde_json::Value {
    let req = srv.requests.lock().unwrap().last().cloned().unwrap();
    serde_json::from_str(&req[req.find("\r\n\r\n").unwrap() + 4..]).unwrap()
}

#[test]
fn nothing_is_sent_before_consent() {
    let srv = FakeServer::start(openai_ok);
    let mut app = app_with(&srv.url(), "consent");
    app.ai.remote_for_tests = true;
    let ctx = egui::Context::default();
    let dir = temp_dir("consent-files");
    select_file(&mut app, &dir, "lib.rs", "fn private_logic() -> u32 { 42 }\n");

    // Asked: the full text is up, and nothing has connected.
    app.run(&ctx, Cmd::AiExplainSelection);
    until(&mut app, &ctx, "the question", |a| a.ai.asking());
    for _ in 0..5 {
        frame(&mut app, &ctx, vec![]);
    }
    let Phase::Asking { prepared, .. } = &app.ai.phase else { panic!() };
    assert!(prepared.prompt.user.contains("fn private_logic() -> u32 { 42 }"));
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(srv.hits.load(Ordering::SeqCst), 0, "nothing before consent");

    // Cancel: still nothing.
    app.ai_answer(&ctx, false, false);
    assert_eq!(app.ai.answer().1, Some("Cancelled: nothing was sent."));
    for _ in 0..5 {
        frame(&mut app, &ctx, vec![]);
    }
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(srv.hits.load(Ordering::SeqCst), 0, "nothing after Cancel");

    // Allow Once: sent once, and asked again next time.
    app.run(&ctx, Cmd::AiExplainSelection);
    until(&mut app, &ctx, "the question", |a| a.ai.asking());
    app.ai_answer(&ctx, true, false);
    until(&mut app, &ctx, "the answer", finished);
    assert_eq!(app.ai.answer(), ("Hello, wörld", None, None));
    assert_eq!(srv.hits.load(Ordering::SeqCst), 1);
    assert!(last_body(&srv)["messages"][1]["content"].as_str().unwrap().contains("fn private_logic"));
    app.run(&ctx, Cmd::AiExplainSelection);
    until(&mut app, &ctx, "the question again", |a| a.ai.asking());
    assert_eq!(srv.hits.load(Ordering::SeqCst), 1);

    // Always: remembered in the config folder, no more questions.
    app.ai_answer(&ctx, true, true);
    until(&mut app, &ctx, "the answer", finished);
    assert_eq!(srv.hits.load(Ordering::SeqCst), 2);
    let saved = std::fs::read_to_string(app.config.as_ref().unwrap().join("ai-consent.json")).unwrap();
    assert!(saved.contains(&format!("openai http://127.0.0.1:{}", srv.port)), "{saved}");
    app.run(&ctx, Cmd::AiExplainSelection);
    until(&mut app, &ctx, "the third answer", |a| finished(a) && !a.ai.answer().0.is_empty());
    assert_eq!(srv.hits.load(Ordering::SeqCst), 3, "no question once allowed");

    // Forgetting asks again.
    app.run(&ctx, Cmd::AiForgetAllowed);
    app.run(&ctx, Cmd::AiExplainSelection);
    until(&mut app, &ctx, "the question after forgetting", |a| a.ai.asking());
    app.ai_cancel();
    assert_eq!(srv.hits.load(Ordering::SeqCst), 3);
}

#[test]
fn off_means_off() {
    let srv = FakeServer::start(openai_ok);
    let mut app = app_with(&srv.url(), "off");
    app.settings.ai_enabled = false;
    let ctx = egui::Context::default();
    let dir = temp_dir("off-files");
    select_file(&mut app, &dir, "a.txt", "some text\n");
    for cmd in [Cmd::AiExplainSelection, Cmd::AiSummarizeChanges, Cmd::AiDraftCommitMessage, Cmd::AiTestConnection] {
        app.run(&ctx, cmd);
        assert!(!app.ai.busy(), "{cmd:?}");
        assert!(app.status.contains("AI helpers are off"), "{}", app.status);
    }
    assert!(app.settings_ui.open, "Settings opens at the AI category");
    for _ in 0..3 {
        frame(&mut app, &ctx, vec![]);
    }
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(srv.hits.load(Ordering::SeqCst), 0);
}

#[test]
fn review_changes_summary_and_commit_message_from_a_local_server() {
    let Some(repo) = temp_repo("ai-ui-review") else { return };
    std::fs::write(repo.join("src/main.rs"), "fn main() {\n    one();\n    three();\n}\n").unwrap();
    let srv = FakeServer::start(openai_ok);
    let mut app = app_with(&srv.url(), "review");
    app.open_folder(repo.clone());
    let ctx = egui::Context::default();
    // Local: no question, straight into the panel.
    app.run(&ctx, Cmd::AiSummarizeChanges);
    assert!(app.ai.visible);
    until(&mut app, &ctx, "the summary", finished);
    assert_eq!(app.ai.answer(), ("Hello, wörld", None, None));
    assert!(app.ai.title.starts_with("Summarize Changes (1 file)"), "{}", app.ai.title);
    assert!(app.ai.provider_label.ends_with("· local"), "{}", app.ai.provider_label);
    let sent = last_body(&srv)["messages"][1]["content"].as_str().unwrap().to_owned();
    assert!(sent.contains("+    three();") && sent.contains("Summarize the changes"), "{sent}");
    // The panel and its Markdown draw.
    for _ in 0..3 {
        frame(&mut app, &ctx, vec![]);
    }
    assert!(!app.ai.blocks.is_empty());

    app.run(&ctx, Cmd::AiDraftCommitMessage);
    until(&mut app, &ctx, "the commit message", finished);
    let sent = last_body(&srv)["messages"][1]["content"].as_str().unwrap().to_owned();
    assert!(sent.contains("Write a Git commit message") && sent.contains("+    three();"), "{sent}");
    // Open in New Tab puts the text in an untitled tab; nothing is committed.
    let tabs = app.docs.len();
    app.ai_open_in_tab();
    assert_eq!(app.docs.len(), tabs + 1);
    assert_eq!(app.docs[app.active].editor.buffer.slice(0..12), "Hello, wörld");
    assert!(app.docs[app.active].path.is_none());

    // Explain Change and Summarize File need a review tab.
    app.run(&ctx, Cmd::AiExplainChange);
    assert!(app.status.contains("Review Changes"), "{}", app.status);
    app.run(&ctx, Cmd::ReviewShowChanges);
    until(&mut app, &ctx, "the scan", |a| a.review.scan.as_ref().is_some_and(|s| !s.files.is_empty()));
    let main = repo.join("src/main.rs");
    app.open_review(&main);
    until(&mut app, &ctx, "the review tab", |a| a.docs.get(a.active).and_then(|d| d.compare.as_ref()?.review.clone()).is_some());
    app.run(&ctx, Cmd::AiExplainChange);
    until(&mut app, &ctx, "the explanation", finished);
    let sent = last_body(&srv)["messages"][1]["content"].as_str().unwrap().to_owned();
    assert!(sent.contains("Explain this change to src/main.rs") && sent.contains("-    two();\n+    three();"), "{sent}");
    app.run(&ctx, Cmd::AiSummarizeFile);
    until(&mut app, &ctx, "the file summary", finished);
    assert!(app.ai.title.contains("src/main.rs"), "{}", app.ai.title);
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn cancel_from_the_ui_and_errors_in_the_panel() {
    let srv = FakeServer::start(|s, _| {
        sse_headers(s);
        send_slowly(s, &[&openai_chunk("partial")]);
        std::thread::sleep(Duration::from_secs(20));
    });
    let mut app = app_with(&srv.url(), "cancel");
    let ctx = egui::Context::default();
    let dir = temp_dir("cancel-files");
    select_file(&mut app, &dir, "a.py", "print('hi')\n");
    app.run(&ctx, Cmd::AiExplainSelection);
    until(&mut app, &ctx, "the first text", |a| a.ai.answer().0 == "partial");
    assert!(app.ai.busy());
    app.run(&ctx, Cmd::AiCancel);
    until(&mut app, &ctx, "the cancel", finished);
    assert_eq!(app.ai.answer(), ("partial", Some(crate::ai::CANCELLED), None), "the text so far stays");

    // A server error lands in the panel.
    let bad = FakeServer::start(|s, _| {
        let body = r#"{"error":{"message":"model 'test-model' not found"}}"#;
        let _ = std::io::Write::write_all(s, format!("HTTP/1.1 404 Not Found\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes());
    });
    app.settings.ai_base_url = bad.url();
    app.run(&ctx, Cmd::AiExplainSelection);
    until(&mut app, &ctx, "the error", finished);
    assert!(app.ai.answer().1.unwrap().contains("model 'test-model' not found"), "{:?}", app.ai.answer());
}

#[test]
fn keys_come_from_the_keychain_and_stay_with_their_host() {
    let srv = FakeServer::start(openai_ok);
    let mut app = app_with(&srv.url(), "keys");
    let ctx = egui::Context::default();
    app.ai_save_key("sk-local-abc");
    let account = format!("openai http://127.0.0.1:{}", srv.port);
    assert_eq!(app.ai.keys().get(&account).unwrap().as_deref(), Some("sk-local-abc"));
    assert!(!std::fs::read_to_string(Settings::path(app.config.as_ref().unwrap())).unwrap_or_default().contains("sk-local"), "never in settings.json");
    let dir = temp_dir("keys-files");
    select_file(&mut app, &dir, "a.txt", "text\n");
    app.run(&ctx, Cmd::AiExplainSelection);
    until(&mut app, &ctx, "the answer", finished);
    assert!(srv.requests.lock().unwrap()[0].contains("Authorization: Bearer sk-local-abc\r\n"));
    // Another host doesn't get it.
    let other = FakeServer::start(openai_ok);
    app.settings.ai_base_url = other.url();
    app.run(&ctx, Cmd::AiExplainSelection);
    until(&mut app, &ctx, "the second answer", finished);
    assert!(!other.requests.lock().unwrap()[0].contains("Authorization"), "a key is only sent where it was saved for");
    // Test Connection from Settings, then remove the key.
    app.settings.ai_base_url = srv.url();
    app.ai_test(&ctx);
    until(&mut app, &ctx, "the test", |a| a.ai.test_result.is_some());
    assert!(app.ai.test_result.as_ref().unwrap().is_ok(), "{:?}", app.ai.test_result);
    app.ai_remove_key();
    assert_eq!(app.ai.keys().get(&account).unwrap(), None);
}

/// Security review, MEDIUM 4: Test Connection asks once before sending a
/// saved key to a remote provider; nothing connects until then.
#[test]
fn test_connection_asks_before_sending_a_key_to_a_remote_provider() {
    let srv = FakeServer::start(|s, _| {
        let body = r#"{"data":[{"id":"test-model"}]}"#;
        let _ = std::io::Write::write_all(s, format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes());
    });
    let mut app = app_with(&srv.url(), "testkey");
    app.ai.remote_for_tests = true;
    let ctx = egui::Context::default();
    // Without a key there's nothing secret to send: no question.
    app.ai_test(&ctx);
    until(&mut app, &ctx, "the keyless test", |a| a.ai.test_result.is_some());
    assert_eq!(srv.hits.load(Ordering::SeqCst), 1);
    app.ai_save_key("sk-remote-xyz");
    app.ai_test(&ctx);
    assert!(app.ai.test_confirm.is_some(), "asked first");
    for _ in 0..3 {
        frame(&mut app, &ctx, vec![]);
    }
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(srv.hits.load(Ordering::SeqCst), 1, "no key sent before the answer");
    app.ai_test_answer(&ctx, false);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(srv.hits.load(Ordering::SeqCst), 1, "nor after Cancel");
    app.ai_test(&ctx);
    app.ai_test_answer(&ctx, true);
    until(&mut app, &ctx, "the test", |a| a.ai.test.is_none() && a.ai.test_result.is_some());
    assert_eq!(srv.hits.load(Ordering::SeqCst), 2);
    assert!(srv.requests.lock().unwrap()[1].contains("Authorization: Bearer sk-remote-xyz\r\n"));
    // Remembered: no question the next time.
    app.ai_test(&ctx);
    assert!(app.ai.test_confirm.is_none());
    until(&mut app, &ctx, "the third test", |a| a.ai.test.is_none());
    assert_eq!(srv.hits.load(Ordering::SeqCst), 3);
}

#[test]
fn the_settings_window_shows_the_ai_row() {
    let mut app = app_with("http://localhost:11434/v1", "settings");
    let ctx = egui::Context::default();
    app.ai_open_settings();
    for _ in 0..3 {
        frame(&mut app, &ctx, vec![]);
    }
    let look = app.ai_look();
    assert_eq!(look.key_for.as_deref(), Some("localhost:11434"));
    assert_eq!(look.key_saved, Some(Ok(false)));
    app.settings.ai_base_url = "http://example.com/v1".into();
    assert!(app.ai_look().problem.unwrap().contains("https"));
}

/// A fake `claude` on PATH: it records its arguments and what it read, and
/// prints an answer, but only once the window has drawn a frame while it
/// runs (so a window that waited for it fails here, however fast the
/// machine). Nothing runs before consent.
#[cfg(unix)]
#[test]
fn command_line_provider_with_a_fake_claude() {
    use std::os::unix::fs::PermissionsExt;
    let bin = temp_dir("fake-bin");
    let script = bin.join("claude");
    let log = bin.join("ran.txt");
    let go = bin.join("go");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{0}.args'\ncat > '{0}'\npwd > '{0}.cwd'\ni=0\nwhile [ ! -e '{1}' ]; do i=$((i+1)); if [ $i -gt 300 ]; then echo 'the window waited for claude' >&2; exit 9; fi; sleep 0.05; done\necho 'Fake **answer**'\n",
            log.display(),
            go.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    // Found on a PATH the way build systems find programs.
    assert_eq!(crate::ai::cli::find_program("claude", Some(bin.clone().into_os_string())), Some(script.clone()));

    let mut app = app_with("http://localhost/v1", "cli");
    app.settings.ai_provider = "claude_cli".into();
    app.ai.program_for_tests = Some(script.clone());
    let ctx = egui::Context::default();
    let dir = temp_dir("cli-files");
    select_file(&mut app, &dir, "x.rs", "fn secret() {}\n");
    app.run(&ctx, Cmd::AiExplainSelection);
    until(&mut app, &ctx, "the question", |a| a.ai.asking());
    std::thread::sleep(Duration::from_millis(200));
    assert!(!log.exists(), "the tool doesn't run before consent");
    app.ai_answer(&ctx, true, false);
    // A frame while it runs (it waits for this); only then may it answer.
    frame(&mut app, &ctx, vec![]);
    assert!(app.ai.busy(), "{:?}", app.ai.answer());
    std::fs::write(&go, "").unwrap();
    until(&mut app, &ctx, "the answer", finished);
    assert_eq!(app.ai.answer(), ("Fake **answer**\n", None, None));
    let stdin = std::fs::read_to_string(&log).unwrap();
    assert!(stdin.contains("fn secret() {}") && stdin.contains("Answer from the text below alone"), "{stdin}");
    let args = std::fs::read_to_string(bin.join("ran.txt.args")).unwrap();
    assert!(args.starts_with("-p\n--tools\n\n"), "{args:?}");
    let cwd = std::fs::read_to_string(bin.join("ran.txt.cwd")).unwrap();
    assert!(cwd.contains("strata-ai-"), "runs in an empty folder of its own: {cwd}");
    assert!(!Path::new(cwd.trim()).exists(), "and the folder is removed afterwards");

    // A failing tool says why.
    std::fs::write(&script, "#!/bin/sh\necho 'Not logged in' >&2\nexit 3\n").unwrap();
    app.run(&ctx, Cmd::AiExplainSelection);
    until(&mut app, &ctx, "the question", |a| a.ai.asking());
    app.ai_answer(&ctx, true, false);
    until(&mut app, &ctx, "the failure", finished);
    assert_eq!(app.ai.answer().1, Some("claude failed (exit 3): Not logged in"));
    // Test Connection runs only --version.
    std::fs::write(&script, "#!/bin/sh\n[ \"$1\" = --version ] && echo '9.9.9 (Claude Code)'\n").unwrap();
    app.ai_test(&ctx);
    until(&mut app, &ctx, "the test", |a| a.ai.test_result.is_some());
    assert!(app.ai.test_result.as_ref().unwrap().as_ref().unwrap().contains("9.9.9 (Claude Code)"), "{:?}", app.ai.test_result);
}

/// Screenshots for the manual and the pull request:
/// `STRATA_AI_SHOTS=<folder> cargo test --release ai_shots` (the consent
/// question, a streamed answer and Settings ▸ AI Helpers, from the fake
/// server). With `STRATA_AI_LIVE_MODEL=<model>` as well, also a real
/// summary of a Review Changes diff from Ollama on localhost:11434.
#[test]
fn ai_shots() {
    use super::super::headless::Shooter;
    let Some(dir) = std::env::var_os("STRATA_AI_SHOTS").map(PathBuf::from) else { return };
    let Some(mut shooter) = Shooter::new(1400, 860, 2.0) else { return };
    let Some(repo) = temp_repo("ai-shots") else { return };
    std::fs::write(repo.join("src/main.rs"), "use std::io;\n\nfn main() {\n    let name = read_name().unwrap_or_else(|| \"world\".into());\n    println!(\"Hello, {name}!\");\n}\n\nfn read_name() -> Option<String> {\n    let mut line = String::new();\n    io::stdin().read_line(&mut line).ok()?;\n    let name = line.trim();\n    (!name.is_empty()).then(|| name.to_owned())\n}\n").unwrap();
    std::fs::write(repo.join("README.md"), "# Title\n\nText.\n\nRun it with `cargo run`.\n").unwrap();
    let answer = "## Overview\n\nThe program now copes with an empty or unreadable name: `read_name` returns an `Option`, and `main` greets the **world** instead of panicking.\n\n- **src/main.rs**: `read_name` returns `Option<String>`, using `?` instead of `unwrap()`; empty input gives `None`.\n- **README.md**: explains how to run the program.\n\nNothing looks unfinished.\n";
    let words: Vec<String> = answer.split_inclusive(' ').map(str::to_owned).collect();
    let srv = FakeServer::start(move |s, _| {
        sse_headers(s);
        for w in &words {
            send_slowly(s, &[&openai_chunk(w)]);
        }
        send_slowly(s, &["data: [DONE]\n\n"]);
    });
    let ctx = egui::Context::default();
    let mut app = app_with(&srv.url(), "shots");
    app.settings.minimap = false;
    app.apply_settings();
    app.open_folder(repo.clone());
    // The fake server stands in for a remote provider: show it as one.
    app.ai.remote_for_tests = true;
    app.ai.shown_host_for_tests = Some("api.example.com".into());
    shooter.run(&ctx, &mut app, 3, vec![]);
    let wait = |shooter: &mut Shooter, app: &mut StrataApp, secs: u64, done: &dyn Fn(&StrataApp) -> bool| {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while !done(app) && Instant::now() < deadline {
            shooter.run(&ctx, app, 1, vec![]);
            std::thread::sleep(Duration::from_millis(25));
        }
    };
    app.run(&ctx, Cmd::AiSummarizeChanges);
    wait(&mut shooter, &mut app, 30, &|a| a.ai.asking());
    shooter.shoot(&ctx, &mut app, 3, vec![], &dir.join("ai_consent.ppm"));
    app.ai_answer(&ctx, true, false);
    wait(&mut shooter, &mut app, 30, &finished);
    shooter.shoot(&ctx, &mut app, 3, vec![], &dir.join("ai_panel.ppm"));
    app.ai_open_settings();
    shooter.shoot(&ctx, &mut app, 4, vec![], &dir.join("ai_settings.ppm"));
    app.close_settings_window();

    if let Some(model) = std::env::var_os("STRATA_AI_LIVE_MODEL").and_then(|m| m.into_string().ok()) {
        app.ai.remote_for_tests = false;
        app.settings.ai_base_url = "http://localhost:11434/v1".into();
        app.settings.ai_model = model;
        app.run(&ctx, Cmd::AiSummarizeChanges);
        wait(&mut shooter, &mut app, 600, &finished);
        let (text, error, _) = app.ai.answer();
        assert!(error.is_none(), "{error:?}");
        assert!(!text.trim().is_empty());
        assert!(app.ai.provider_label.ends_with("· local"));
        shooter.shoot(&ctx, &mut app, 3, vec![], &dir.join("ai_live_ollama.ppm"));
    }
    let _ = std::fs::remove_dir_all(&repo);
}
