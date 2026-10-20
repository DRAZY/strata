//! Plugins through the window: the consent dialog, every window method
//! with the fake plugin (`plugins::fake`), edits undone byte for byte, pick
//! lists, the panel, status items, the palette and menu, installing, and
//! the two example plugins in `examples/plugins` (with python3 and node,
//! when they're installed).

use super::*;
use crate::app::ui_tests::temp_dir;
use crate::plugins::api::{DOCUMENT_CHANGED, NO_DOCUMENT, PERMISSION_DENIED};
use crate::plugins::manager::tests::install_fake;
use egui::{Context, Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, pos2, vec2};
use std::time::{Duration, Instant};

/// One frame; the labelled accessibility nodes: (label, rect).
fn frame(app: &mut StrataApp, ctx: &Context, events: Vec<Event>) -> Vec<(String, Rect)> {
    ctx.enable_accesskit();
    let time = Some(ctx.input(|i| i.time) + 0.1);
    let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 760.0))), events, time, focused: true, ..Default::default() };
    let mut out = ctx.run_ui(raw, |ui| {
        let mut frame = eframe::Frame::_new_kittest();
        eframe::App::ui(app, ui, &mut frame);
    });
    out.textures_delta.clear();
    let Some(update) = out.platform_output.accesskit_update else { return Vec::new() };
    update
        .nodes
        .iter()
        .filter_map(|(_, n)| {
            // Buttons carry their text as a label, plain text as a value.
            let label = n.label().or_else(|| n.value())?.to_owned();
            let b = n.bounds()?;
            Some((label, Rect::from_min_max(pos2(b.x0 as f32, b.y0 as f32), pos2(b.x1 as f32, b.y1 as f32))))
        })
        .collect()
}

/// Frames until `f` says yes (up to 20 s); the last frame's nodes.
fn until(app: &mut StrataApp, ctx: &Context, what: &str, mut f: impl FnMut(&mut StrataApp, &[(String, Rect)]) -> bool) -> Vec<(String, Rect)> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let nodes = frame(app, ctx, vec![]);
        if f(app, &nodes) {
            return nodes;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}; status: {:?}", app.status);
        std::thread::sleep(Duration::from_millis(15));
    }
}

fn has(nodes: &[(String, Rect)], needle: &str) -> bool {
    nodes.iter().any(|(l, _)| l.contains(needle))
}

/// Click the node labelled exactly `label`.
fn click(app: &mut StrataApp, ctx: &Context, label: &str) {
    until(app, ctx, label, |_, n| n.iter().any(|(l, _)| l == label));
    // A dialog that just opened takes a frame or two to be laid out.
    frame(app, ctx, vec![]);
    let nodes = frame(app, ctx, vec![]);
    let at = nodes.iter().find(|(l, _)| l == label).unwrap_or_else(|| panic!("{label} went away")).1.center();
    let b = |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    frame(app, ctx, vec![Event::PointerMoved(at), b(true)]);
    frame(app, ctx, vec![b(false)]);
    frame(app, ctx, vec![]);
}

fn text(app: &StrataApp) -> String {
    let b = &app.docs[app.active].editor.buffer;
    b.slice(0..b.len_chars())
}

fn key(k: Key, modifiers: Modifiers) -> Event {
    Event::Key { key: k, physical_key: Some(k), pressed: true, repeat: false, modifiers }
}

/// ⌘ (Ctrl elsewhere) as a real key press reports it, plus `more`.
fn primary(more: Modifiers) -> Modifiers {
    let base = if crate::keys::MAC { Modifiers::MAC_CMD.plus(Modifiers::COMMAND) } else { Modifiers::CTRL.plus(Modifiers::COMMAND) };
    base.plus(more)
}

fn installed(cmd: &str) -> bool {
    std::process::Command::new(cmd).arg("--version").output().is_ok_and(|o| o.status.success())
}

/// A window on `dir` with `file` open (written with `content`), plugins from `config`.
fn window(ctx: &Context, dir: &Path, config: &Path, file: &str, content: &str) -> StrataApp {
    let path = dir.join(file);
    std::fs::write(&path, content).unwrap();
    let mut app = StrataApp::new(Some(dir.to_owned()), Some(config.to_owned()));
    app.terminal_started = true;
    app.open_path(path, false);
    frame(&mut app, ctx, vec![]);
    app
}

/// Run the fake plugin's `cmd` on the current tab, answering through the
/// window: what the plugin got back.
fn fake(app: &mut StrataApp, ctx: &Context, cmd: &str, args: Value) -> Value {
    let doc = app.docs[app.active].editor.id();
    let mut p = app.plugins.mgr.borrow().request("fake", "command/execute", json!({"command": cmd, "document": {"id": doc}, "args": args})).expect("the fake plugin runs");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        frame(app, ctx, vec![]);
        if let Some(r) = p.poll() {
            return r.expect("the fake plugin answers");
        }
        assert!(Instant::now() < deadline, "timed out waiting for {cmd}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Start the fake plugin, answering the consent question with a click.
fn start_fake(app: &mut StrataApp, ctx: &Context) {
    app.plugins.mgr.borrow_mut().start("fake");
    click(app, ctx, "Allow and Start");
    until(app, ctx, "the plugin to run", |a, _| a.plugins.mgr.borrow().plugins.iter().any(|p| p.name == "fake" && matches!(p.state, State::Running)));
}

#[test]
fn the_consent_dialog_lists_permissions_in_plain_words() {
    let dir = temp_dir("plugin-consent");
    let config = dir.join("config");
    install_fake(&config, "fake", &[], &["read_document", "run_process", "network"], json!({"description": "Does fake things"}));
    let ctx = Context::default();
    let mut app = window(&ctx, &dir, &config, "a.txt", "hello\n");
    app.plugins.mgr.borrow_mut().start("fake");
    let nodes = until(&mut app, &ctx, "the dialog", |_, n| has(n, "Allow the Fake plugin to run?"));
    for want in [
        "Does fake things",
        "Read the text of the files you have open",
        "Run any program on your computer (Strata can't limit this)",
        "Reach the internet and computers on your network (Strata can't limit this)",
        "can't sandbox the program itself",
        "If any of its files change, Strata asks again",
    ] {
        assert!(has(&nodes, want), "missing {want:?}");
    }
    assert!(app.modal_open(&ctx), "the dialog has the keyboard");
    // Esc is "Don't Start".
    frame(&mut app, &ctx, vec![key(Key::Escape, Modifiers::NONE)]);
    let nodes = until(&mut app, &ctx, "the dialog to close", |_, n| !has(n, "Allow the Fake plugin"));
    let _ = nodes;
    assert!(app.status.contains("wasn't started"), "{}", app.status);
    assert!(app.plugins.mgr.borrow().plugins[0].pid().is_none());
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn documents_are_read_and_edited_through_the_editor_with_undo() {
    let dir = temp_dir("plugin-edit");
    let config = dir.join("config");
    install_fake(&config, "fake", &[], &["read_document", "edit_document"], json!({}));
    let original = "héllo 👋\r\nsecond line\r\n";
    let ctx = Context::default();
    let mut app = window(&ctx, &dir, &config, "a.txt", original);
    start_fake(&mut app, &ctx);
    // document/read: the text, the selections (in code points) and the line ending.
    app.docs[app.active].editor.select_ranges(&[2..7], 0);
    let r = fake(&mut app, &ctx, "read", json!({}));
    let res = &r["result"];
    assert_eq!(res["text"], original);
    assert_eq!(res["lineEnding"], "\r\n");
    assert_eq!(res["document"]["name"], "a.txt");
    assert_eq!(res["selections"][0]["start"], json!({"line": 0, "character": 2}));
    assert_eq!(res["selections"][0]["end"], json!({"line": 0, "character": 7}));
    // document/applyEdits: one undo step, and Undo puts back every byte.
    let before_steps = app.docs[app.active].editor.history_len();
    let r = fake(&mut app, &ctx, "upper", json!({}));
    assert_eq!(r["result"]["applied"], true, "{r}");
    assert_eq!(text(&app), "HÉLLO 👋\r\nSECOND LINE\r\n");
    assert_eq!(app.docs[app.active].editor.history_len(), before_steps + 1, "one undo step");
    app.docs[app.active].editor.undo();
    assert_eq!(text(&app), original, "byte-exact after Undo");
    app.docs[app.active].editor.redo();
    assert_eq!(text(&app), "HÉLLO 👋\r\nSECOND LINE\r\n");
    app.docs[app.active].editor.undo();
    // An edit made for an older version is refused, and nothing changes.
    let r = fake(&mut app, &ctx, "stale", json!({}));
    assert_eq!(r["error"]["code"], DOCUMENT_CHANGED, "{r}");
    assert_eq!(text(&app), original);
    // A read-only tab.
    app.docs[app.active].editor.read_only = true;
    let r = fake(&mut app, &ctx, "upper", json!({}));
    assert_eq!(r["error"]["code"], crate::plugins::api::READ_ONLY, "{r}");
    app.docs[app.active].editor.read_only = false;
    // A document that isn't open, and bad edits.
    let r = fake(&mut app, &ctx, "call", json!({"method": "document/read", "params": {"document": 999_999}}));
    assert_eq!(r["error"]["code"], NO_DOCUMENT);
    let id = app.docs[app.active].editor.id();
    let version = app.docs[app.active].editor.version();
    let overlap = json!([{"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 3}}, "newText": "a"}, {"range": {"start": {"line": 0, "character": 2}, "end": {"line": 0, "character": 4}}, "newText": "b"}]);
    let r = fake(&mut app, &ctx, "call", json!({"method": "document/applyEdits", "params": {"document": id, "version": version, "edits": overlap}}));
    assert_eq!(r["error"]["code"], crate::plugins::api::INVALID_PARAMS);
    assert_eq!(text(&app), original);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn panels_messages_status_items_and_pick_lists() {
    let dir = temp_dir("plugin-ui");
    let config = dir.join("config");
    install_fake(&config, "fake", &[], &["panel", "notify"], json!({}));
    let ctx = Context::default();
    let mut app = window(&ctx, &dir, &config, "a.txt", "x\n");
    start_fake(&mut app, &ctx);
    // ui/showPanel: Markdown in a panel at the bottom.
    assert_eq!(fake(&mut app, &ctx, "panel", json!({})), json!({"result": null}));
    until(&mut app, &ctx, "the panel", |_, n| has(n, "Fake: Fake Panel"));
    assert!(app.plugins.panel.as_ref().is_some_and(|p| !p.blocks.is_empty() && p.text.is_none()), "Markdown, parsed");
    click(&mut app, &ctx, "Close");
    assert!(app.plugins.panel.is_none());
    // ui/notify: the status bar, named after the plugin.
    fake(&mut app, &ctx, "notify", json!({}));
    assert_eq!(app.status, "Fake: Hello from the fake plugin");
    // ui/statusItem.
    fake(&mut app, &ctx, "status", json!({}));
    until(&mut app, &ctx, "the status item", |_, n| has(n, "Fake: 3 words"));
    // ui/pick: the palette's list; ↓ Enter picks the second.
    let mut p = app.plugins.mgr.borrow().request("fake", "command/execute", json!({"command": "pick"})).unwrap();
    until(&mut app, &ctx, "the list", |a, _| a.palette.open);
    assert_eq!(app.palette.choice_labels(), ["one", "two — the second", "three"]);
    // A second list while one is open: busy.
    let r = fake_busy(&mut app, &ctx);
    assert_eq!(r["error"]["code"], crate::plugins::api::BUSY, "{r}");
    frame(&mut app, &ctx, vec![key(Key::ArrowDown, Modifiers::NONE)]);
    frame(&mut app, &ctx, vec![key(Key::Enter, Modifiers::NONE)]);
    let r = poll_until(&mut app, &ctx, &mut p);
    assert_eq!(r, json!({"result": {"index": 1}}));
    // Esc: no choice.
    let mut p = app.plugins.mgr.borrow().request("fake", "command/execute", json!({"command": "pick"})).unwrap();
    until(&mut app, &ctx, "the list", |a, _| a.palette.open);
    frame(&mut app, &ctx, vec![key(Key::Escape, Modifiers::NONE)]);
    assert_eq!(poll_until(&mut app, &ctx, &mut p), json!({"result": null}));
    std::fs::remove_dir_all(dir).ok();
}

/// While a pick list is open, the fake plugin asks for another.
fn fake_busy(app: &mut StrataApp, ctx: &Context) -> Value {
    let mut p = app.plugins.mgr.borrow().request("fake", "command/execute", json!({"command": "pick"})).unwrap();
    poll_until(app, ctx, &mut p)
}

fn poll_until(app: &mut StrataApp, ctx: &Context, p: &mut crate::plugins::host::Pending) -> Value {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        frame(app, ctx, vec![]);
        if let Some(r) = p.poll() {
            return r.unwrap();
        }
        assert!(Instant::now() < deadline, "timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn window_methods_are_refused_without_their_permission() {
    let dir = temp_dir("plugin-refused");
    let config = dir.join("config");
    install_fake(&config, "fake", &[], &[], json!({}));
    let ctx = Context::default();
    let mut app = window(&ctx, &dir, &config, "a.txt", "keep me\n");
    start_fake(&mut app, &ctx);
    let id = app.docs[app.active].editor.id();
    let edit = json!([{"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 4}}, "newText": "lose"}]);
    let calls = [
        ("document/read", json!({"document": id})),
        ("document/applyEdits", json!({"document": id, "version": app.docs[app.active].editor.version(), "edits": edit})),
        ("ui/showPanel", json!({"title": "t", "markdown": "# hi"})),
        ("ui/notify", json!({"message": "hi"})),
        ("ui/statusItem", json!({"id": "s", "text": "hi"})),
        ("ui/pick", json!({"items": ["a"]})),
    ];
    for (method, params) in calls {
        let r = fake(&mut app, &ctx, "call", json!({"method": method, "params": params}));
        assert_eq!(r["error"]["code"], PERMISSION_DENIED, "{method}: {r}");
    }
    assert_eq!(text(&app), "keep me\n");
    assert!(app.plugins.panel.is_none() && !app.palette.open);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn plugin_commands_are_in_the_palette_and_menu_and_bound_keys_run_them() {
    let dir = temp_dir("plugin-cmds");
    let config = dir.join("config");
    let extra = json!({"contributes": {"commands": [{"id": "upper", "title": "Shout the File"}, {"id": "crash", "title": "Crash", "menu": false}], "keybindings": [{"command": "upper", "key": "primary+alt+shift+8"}, {"command": "crash", "key": "primary+s"}]}});
    install_fake(&config, "fake", &[], &["read_document", "edit_document"], extra);
    let ctx = Context::default();
    let mut app = window(&ctx, &dir, &config, "a.txt", "quiet\n");
    // Palette entries, with the free key; the taken one (⌘S, Save) isn't bound.
    let items = app.plugin_palette_items();
    let shout = items.iter().find(|i| i.0 == "Fake: Shout the File").expect("listed");
    assert_eq!(shout.1, "plugin:fake:upper");
    assert!(!shout.2.is_empty(), "shows its key");
    assert!(items.iter().find(|i| i.0 == "Fake: Crash").unwrap().2.is_empty(), "Strata keeps ⌘S");
    // Tools ▸ Plugins ▸ Fake ▸ Shout the File (Crash asked not to be in the menu).
    let Item::Sub(label, kids) = app.plugins_menu() else { panic!() };
    assert_eq!(label, "Plugins");
    let fake_menu = kids.iter().find_map(|k| match k {
        Item::Sub(l, items) if l == "Fake" => Some(items),
        _ => None,
    });
    let labels: Vec<&str> = fake_menu.unwrap().iter().filter_map(|i| if let Item::Action { label, .. } = i { Some(label.as_str()) } else { None }).collect();
    assert_eq!(labels, ["Shout the File"]);
    // Its key starts the plugin (after the question) and runs the command.
    frame(&mut app, &ctx, vec![key(Key::Num8, primary(Modifiers::ALT.plus(Modifiers::SHIFT)))]);
    click(&mut app, &ctx, "Allow and Start");
    until(&mut app, &ctx, "the edit", |a, _| text(a) == "QUIET\n");
    // The keyboard is the editor's again after the question (Undo, typing).
    frame(&mut app, &ctx, vec![]);
    assert!(app.docs[app.active].view.has_focus(&ctx), "the editor has the keyboard");
    // The palette runs it too.
    app.docs[app.active].editor.undo();
    assert_eq!(text(&app), "quiet\n");
    app.palette_events(&ctx, vec![crate::palette::Event::Named("plugin:fake:upper".into())]);
    until(&mut app, &ctx, "the edit from the palette", |a, _| text(a) == "QUIET\n");
    // The manager shows the plugin, its key problem and its log.
    app.plugins.manager_open = true;
    let nodes = until(&mut app, &ctx, "the manager", |_, n| has(n, "Key primary+s isn't bound"));
    assert!(has(&nodes, "Status: Running") && has(&nodes, "Log") && has(&nodes, "Changed a.txt (1 edits)"), "{:?}", nodes.iter().map(|n| &n.0).collect::<Vec<_>>());
    std::fs::remove_dir_all(dir).ok();
}

/// The repository's example plugins.
fn example(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/plugins").join(name)
}

fn install(app: &mut StrataApp, ctx: &Context, path: PathBuf, zip: bool) {
    app.plugin_install_from(ctx, path, zip);
    until(app, ctx, "the install", |a, _| !a.plugin_installing_now());
}

#[test]
fn the_word_count_example_counts_as_you_type() {
    if !installed("python3") {
        eprintln!("python3 isn't installed: skipped");
        return;
    }
    let dir = temp_dir("plugin-wc");
    let config = dir.join("config");
    let ctx = Context::default();
    let mut app = window(&ctx, &dir, &config, "notes.md", "one two three\n");
    install(&mut app, &ctx, example("word-count"), false);
    assert_eq!(app.status, "Installed Word Count 1.0.0");
    assert!(config.join("Plugins/word-count/word_count.py").is_file());
    // Installing opens the manager; the open .md file asks to start it.
    let n = app.docs.len();
    app.open_path(dir.join("other.md"), false);
    std::fs::write(dir.join("other.md"), "").unwrap();
    let _ = n;
    click(&mut app, &ctx, "Allow and Start");
    let i = app.docs.iter().position(|d| d.name() == "notes.md").unwrap();
    app.activate(i);
    until(&mut app, &ctx, "the count", |_, n| has(n, "Word Count: 3"));
    app.docs[app.active].editor.insert_text("four five ");
    until(&mut app, &ctx, "the new count", |_, n| has(n, "Word Count: 5"));
    // Its setting from settings.json.
    app.settings.plugins = serde_json::from_value(json!({"word-count": {"count_numbers": false}})).unwrap();
    app.docs[app.active].editor.insert_text("42 six ");
    // Counts made before the change may land first; it settles on 6 (7 if 42 counted).
    until(&mut app, &ctx, "numbers left out", |_, n| has(n, "Word Count: 6"));
    // Its command, from the palette, shows a panel.
    app.palette_events(&ctx, vec![crate::palette::Event::Named("plugin:word-count:show".into())]);
    until(&mut app, &ctx, "the panel", |_, n| has(n, "Word Count: notes.md"));
    assert!(app.plugins.panel.as_ref().is_some_and(|p| p.blocks.iter().any(|b| matches!(b.block, crate::markdown::Block::Table { .. }))), "the table");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn the_sort_unique_example_sorts_and_undo_puts_it_back() {
    if !installed("node") {
        eprintln!("node isn't installed: skipped");
        return;
    }
    let dir = temp_dir("plugin-sort");
    let config = dir.join("config");
    let ctx = Context::default();
    let original = "pear\r\napple\r\npear\r\nfig\r\n";
    let mut app = window(&ctx, &dir, &config, "fruit.txt", original);
    install(&mut app, &ctx, example("sort-unique"), false);
    // Its key binding: primary+alt+shift+U.
    frame(&mut app, &ctx, vec![key(Key::U, primary(Modifiers::ALT.plus(Modifiers::SHIFT)))]);
    click(&mut app, &ctx, "Allow and Start");
    until(&mut app, &ctx, "the sort", |a, _| text(a) == "apple\r\nfig\r\npear\r\n");
    until(&mut app, &ctx, "its message", |a, _| a.status.contains("removed 1 duplicate line"));
    app.docs[app.active].editor.undo();
    assert_eq!(text(&app), original, "byte-exact after Undo");
    // Just the selected lines.
    app.docs[app.active].editor.select_ranges(&[0..17], 0);
    app.plugin_act("plugin:sort-unique:sort-unique");
    until(&mut app, &ctx, "the selection sorted", |a, _| text(a) == "apple\r\npear\r\npear\r\nfig\r\n" || text(a) == "apple\r\npear\r\nfig\r\n");
    assert_eq!(text(&app), "apple\r\npear\r\nfig\r\n", "the two selected lines sorted, the duplicate third line removed");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn installing_from_a_zip_and_refusing_a_bad_one() {
    use crate::plugins::zip::build::{Item as Z, zip};
    let dir = temp_dir("plugin-zip");
    let config = dir.join("config");
    let ctx = Context::default();
    let mut app = window(&ctx, &dir, &config, "a.txt", "");
    let manifest = std::fs::read(example("word-count").join("strata-plugin.json")).unwrap();
    let script = std::fs::read(example("word-count").join("word_count.py")).unwrap();
    let good = zip(&[Z::file("word-count/strata-plugin.json", &manifest), Z::file("word-count/word_count.py", &script)]);
    std::fs::write(dir.join("good.zip"), good).unwrap();
    install(&mut app, &ctx, dir.join("good.zip"), true);
    assert_eq!(app.status, "Installed Word Count 1.0.0");
    assert_eq!(std::fs::read(config.join("Plugins/word-count/word_count.py")).unwrap(), script);
    // A zip that writes outside the package: refused, nothing written.
    let bad = zip(&[Z::file("strata-plugin.json", &manifest), Z::file("../../escaped.py", b"print('x')")]);
    std::fs::write(dir.join("bad.zip"), bad).unwrap();
    install(&mut app, &ctx, dir.join("bad.zip"), true);
    assert!(app.status.starts_with("Couldn't install the plugin:") && app.status.contains("outside the package"), "{}", app.status);
    assert!(!dir.join("escaped.py").exists() && !config.join("escaped.py").exists());
    // Removing it.
    app.plugin_remove("word-count");
    assert_eq!(app.status, "Removed Word Count");
    assert!(!config.join("Plugins/word-count").exists());
    std::fs::remove_dir_all(dir).ok();
}


/// No drawn label or value holds a raw U+202E (a marker or `[RLO]` instead).
fn no_raw_rlo(nodes: &[(String, Rect)], step: &str) {
    let raw: Vec<&String> = nodes.iter().map(|n| &n.0).filter(|l| l.contains('\u{202E}')).collect();
    assert!(raw.is_empty(), "{step}: a raw direction override reached the screen: {raw:?}");
}

#[test]
fn direction_overrides_from_a_plugin_never_reach_the_screen_raw() {
    let dir = temp_dir("plugin-rlo");
    let config = dir.join("config");
    let evil = json!({
        "title": "Evil\u{202E}Name",
        "description": "Looks \u{202E}harmless",
        "author": "Some\u{202E}one",
        "contributes": {"commands": [{"id": "read", "title": "Read\u{202E} Me"}], "settings": {"mode": {"default": "a\u{202E}b", "description": "x\u{202E}y"}}}
    });
    install_fake(&config, "fake", &[], &["panel", "notify"], evil);
    let ctx = Context::default();
    let mut app = window(&ctx, &dir, &config, "a.txt", "x\n");
    // The consent dialog: title, author, description.
    app.plugins.mgr.borrow_mut().start("fake");
    let nodes = until(&mut app, &ctx, "the dialog", |_, n| has(n, "Allow the Evil"));
    no_raw_rlo(&nodes, "consent");
    assert!(has(&nodes, "Evil[RLO]Name") && has(&nodes, "Looks [RLO]harmless"), "spelled out");
    click(&mut app, &ctx, "Allow and Start");
    until(&mut app, &ctx, "the plugin to run", |a, _| a.plugins.mgr.borrow().plugins.iter().any(|p| matches!(p.state, State::Running)));
    // Menus take plain strings.
    let Item::Sub(_, kids) = app.plugins_menu() else { panic!() };
    let sub = kids.iter().find_map(|k| if let Item::Sub(l, items) = k { Some((l.clone(), items)) } else { None }).unwrap();
    assert_eq!(sub.0, "Evil[RLO]Name");
    assert!(matches!(&sub.1[0], Item::Action { label, .. } if label == "Read[RLO] Me"));
    // A status item, a message, a panel (its title and plain text).
    let rlo = "\u{202E}";
    fake(&mut app, &ctx, "call", json!({"method": "ui/statusItem", "params": {"id": "s", "text": format!("count{rlo}321"), "tooltip": format!("t{rlo}ip")}}));
    fake(&mut app, &ctx, "call", json!({"method": "ui/notify", "params": {"message": format!("note{rlo}evil")}}));
    fake(&mut app, &ctx, "call", json!({"method": "ui/showPanel", "params": {"title": format!("panel{rlo}title"), "text": format!("body{rlo}text")}}));
    let nodes = until(&mut app, &ctx, "the panel", |_, n| has(n, "panel") && has(n, "count") && has(n, "body"));
    no_raw_rlo(&nodes, "status item, message and panel");
    assert!(has(&nodes, "RLO"), "markers drawn");
    // A pick list: its rows and hint.
    let mut p = app.plugins.mgr.borrow().request("fake", "command/execute", json!({"command": "call", "args": {"method": "ui/pick", "params": {"items": [format!("row{rlo}one")], "placeholder": format!("pick{rlo}hint")}}})).unwrap();
    let nodes = until(&mut app, &ctx, "the list", |a, _| a.palette.open);
    no_raw_rlo(&nodes, "pick");
    assert!(!crate::hidden::any(&crate::palette::highlighted_for_tests(&app.palette.choice_labels()[0])), "pick row");
    frame(&mut app, &ctx, vec![key(Key::Escape, Modifiers::NONE)]);
    poll_until(&mut app, &ctx, &mut p);
    // The palette's row for the command.
    let item = app.plugin_palette_items().into_iter().find(|i| i.1 == "plugin:fake:read").unwrap();
    assert!(!crate::hidden::any(&crate::palette::highlighted_for_tests(&item.0)), "palette row");
    // The manager: names, description, settings, the log.
    app.plugins.mgr.borrow().log("fake", format!("log{rlo}line"));
    app.plugins.manager_open = true;
    let nodes = until(&mut app, &ctx, "the manager", |_, n| has(n, "Copy Log") && has(n, "log[RLO]line"));
    no_raw_rlo(&nodes, "manager");
    assert!(has(&nodes, "Evil[RLO]Name") && has(&nodes, "x[RLO]y"));
    std::fs::remove_dir_all(dir).ok();
}
