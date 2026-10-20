//! Whole-frame checks of window behavior: dialogs and the keyboard, focus,
//! the tab strip, the side bar.

use super::*;
use egui::{Context, Event, Key, Modifiers, Pos2, RawInput};

pub(super) fn frame_sized(app: &mut StrataApp, ctx: &Context, size: egui::Vec2, events: Vec<Event>) -> egui::FullOutput {
    // Time moves on (a quarter second a frame) so animations finish.
    let time = Some(ctx.input(|i| i.time) + 0.25);
    let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)), events, time, ..Default::default() };
    let mut out = ctx.run_ui(raw, |ui| {
        let mut frame = eframe::Frame::_new_kittest();
        eframe::App::ui(app, ui, &mut frame);
    });
    out.textures_delta.clear();
    out
}

pub(super) fn frame(app: &mut StrataApp, ctx: &Context, events: Vec<Event>) -> egui::FullOutput {
    frame_sized(app, ctx, vec2(1000.0, 600.0), events)
}

/// One frame's labelled accessibility nodes: (label, rect).
pub(super) fn labelled(app: &mut StrataApp, ctx: &Context) -> Vec<(String, Rect)> {
    ctx.enable_accesskit();
    frame(app, ctx, vec![]);
    let out = frame(app, ctx, vec![]);
    let Some(update) = out.platform_output.accesskit_update else { return Vec::new() };
    update
        .nodes
        .iter()
        .filter_map(|(_, n)| {
            let b = n.bounds()?;
            Some((n.label()?.to_owned(), Rect::from_min_max(egui::pos2(b.x0 as f32, b.y0 as f32), egui::pos2(b.x1 as f32, b.y1 as f32))))
        })
        .collect()
}

/// One frame's text on screen: labelled nodes and plain labels (whose
/// text accesskit carries as their value).
pub(super) fn shown_text(app: &mut StrataApp, ctx: &Context) -> Vec<String> {
    ctx.enable_accesskit();
    frame(app, ctx, vec![]);
    let out = frame(app, ctx, vec![]);
    let Some(update) = out.platform_output.accesskit_update else { return Vec::new() };
    update.nodes.iter().filter_map(|(_, n)| n.label().or(n.value()).map(str::to_owned)).collect()
}

fn primary() -> Modifiers {
    if cfg!(target_os = "macos") { Modifiers::MAC_CMD | Modifiers::COMMAND } else { Modifiers::CTRL | Modifiers::COMMAND }
}

pub(super) fn press(key: Key, modifiers: Modifiers) -> Event {
    Event::Key { key, physical_key: Some(key), pressed: true, repeat: false, modifiers }
}

pub(super) fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("strata-ui-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn shortcuts_wait_while_a_dialog_is_up() {
    let dir = temp_dir("modal");
    let (a, b) = (dir.join("a.txt"), dir.join("b.txt"));
    std::fs::write(&a, "a\n").unwrap();
    std::fs::write(&b, "b\n").unwrap();
    let ctx = Context::default();
    let mut app = StrataApp::new(Some(a.clone()), None);
    app.open_folder(dir.clone());
    app.open_path(b.clone(), false);
    for _ in 0..3 {
        frame(&mut app, &ctx, vec![]);
    }
    app.sidebar.ask_rename(&b);
    frame(&mut app, &ctx, vec![]);
    let name = |app: &mut StrataApp| app.sidebar.prompt_state().map(|(n, _)| n.clone());
    assert_eq!(name(&mut app).as_deref(), Some("b.txt"), "the rename dialog is up");
    frame(&mut app, &ctx, vec![press(Key::W, primary())]);
    assert_eq!(app.docs.len(), 2, "Cmd+W didn't close the tab behind the dialog");
    frame(&mut app, &ctx, vec![press(Key::P, primary() | Modifiers::SHIFT)]);
    assert!(!app.palette.open, "nor did Cmd+Shift+P open the palette");
    frame(&mut app, &ctx, vec![press(Key::F, primary())]);
    assert!(!app.find.open);
    // Typing goes to the name field, not the editor; the dialog opened with
    // the name selected up to the extension. (What the shortcuts did to the
    // field itself is egui's: off macOS Ctrl+W deletes that word.)
    frame(&mut app, &ctx, vec![Event::Text("x".into())]);
    assert_eq!(name(&mut app).as_deref(), Some("x.txt"));
    assert_eq!(app.docs[app.active].editor.buffer.line_text(0), "b");
    // One Esc closes the dialog only.
    app.open_find(false);
    frame(&mut app, &ctx, vec![]);
    frame(&mut app, &ctx, vec![press(Key::Escape, Modifiers::NONE)]);
    assert!(app.sidebar.prompt_state().is_none(), "Esc closed the dialog");
    assert!(app.find.open, "and left the find bar alone");
    // Shortcuts work again.
    frame(&mut app, &ctx, vec![press(Key::P, primary() | Modifiers::SHIFT)]);
    assert!(app.palette.open);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn print_dialog_holds_the_keyboard_too() {
    let ctx = Context::default();
    let mut app = StrataApp::new(None, None);
    app.docs[0].editor.insert_text("hello");
    frame(&mut app, &ctx, vec![]);
    app.open_print(printing::Output::Pdf);
    frame(&mut app, &ctx, vec![]);
    frame(&mut app, &ctx, vec![press(Key::N, primary())]);
    assert_eq!(app.docs.len(), 1, "Cmd+N waited");
    frame(&mut app, &ctx, vec![press(Key::Escape, Modifiers::NONE)]);
    assert!(!app.print.is_open());
}

#[test]
fn find_bar_keeps_focus_after_the_palette_closes() {
    let ctx = Context::default();
    let mut app = StrataApp::new(None, None);
    app.docs[0].editor.insert_text("hello world\nsecond line\n");
    for _ in 0..3 {
        frame(&mut app, &ctx, vec![]);
    }
    frame(&mut app, &ctx, vec![press(Key::P, primary() | Modifiers::SHIFT)]);
    frame(&mut app, &ctx, vec![]);
    assert!(app.palette.open);
    frame(&mut app, &ctx, vec![press(Key::Escape, Modifiers::NONE)]);
    assert!(!app.palette.open);
    // The palette's close queued editor focus; Cmd+F in the very next frame wins.
    frame(&mut app, &ctx, vec![press(Key::F, primary())]);
    for _ in 0..3 {
        frame(&mut app, &ctx, vec![]);
    }
    frame(&mut app, &ctx, vec![Event::Text("line".into())]);
    assert!(app.find.open);
    assert_eq!(app.find.query.text, "line", "typed into the find field");
    assert_eq!(app.docs[0].editor.buffer.line_text(0), "hello world", "not into the editor");
}

/// A find field opened with text in it has that text selected, so typing
/// replaces it; after Return the field keeps the keyboard.
#[test]
fn find_fields_open_with_their_text_selected() {
    let dir = temp_dir("find-select");
    std::fs::write(dir.join("a.txt"), "hello world\n").unwrap();
    let ctx = Context::default();
    let mut app = StrataApp::new(None, None);
    app.open_folder(dir.clone());
    app.open_path(dir.join("a.txt"), false);
    for _ in 0..3 {
        frame(&mut app, &ctx, vec![]);
    }
    // Find: the query from last time is selected.
    app.find.query.text = "earlier".into();
    frame(&mut app, &ctx, vec![press(Key::F, primary())]);
    frame(&mut app, &ctx, vec![]);
    frame(&mut app, &ctx, vec![Event::Text("wor".into())]);
    assert_eq!(app.find.query.text, "wor", "typing replaced the query");
    frame(&mut app, &ctx, vec![press(Key::Escape, Modifiers::NONE)]);
    assert!(!app.find.open);

    // Find in Files: prefilled from the selection, selected too.
    app.docs[app.active].editor.select_ranges(&[0..5], 0);
    frame(&mut app, &ctx, vec![press(Key::F, primary() | Modifiers::SHIFT)]);
    frame(&mut app, &ctx, vec![]);
    assert_eq!(app.sidebar.query.text, "hello");
    frame(&mut app, &ctx, vec![Event::Text("wor".into())]);
    assert_eq!(app.sidebar.query.text, "wor", "typing replaced the prefill");
    // Return searches and leaves the keyboard in the field.
    frame(&mut app, &ctx, vec![press(Key::Enter, Modifiers::NONE)]);
    for _ in 0..3 {
        frame(&mut app, &ctx, vec![]);
    }
    frame(&mut app, &ctx, vec![press(Key::A, primary())]);
    frame(&mut app, &ctx, vec![Event::Text("hel".into())]);
    assert_eq!(app.sidebar.query.text, "hel", "Select All and typing still reach the field");
    assert_eq!(app.docs[app.active].editor.buffer.line_text(0), "hello world");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn toggle_side_bar_check_matches_what_is_shown() {
    use crate::menus::Item;
    fn checked(app: &StrataApp) -> Option<bool> {
        fn find(items: &[Item]) -> Option<Option<bool>> {
            items.iter().find_map(|it| match it {
                Item::Action { label, check, .. } if label == "Toggle Side Bar" => Some(*check),
                Item::Sub(_, kids) => find(kids),
                _ => None,
            })
        }
        app.menus().iter().find_map(|(_, items)| find(items)).flatten()
    }
    let ctx = Context::default();
    let mut app = StrataApp::new(None, None);
    assert!(!app.sidebar.shown());
    assert_eq!(checked(&app), Some(false), "no folder: nothing shown, no check");
    // With no folder open it still comes out, with the open files and the file's folder.
    let dir = temp_dir("toggle");
    std::fs::write(dir.join("a.txt"), "a").unwrap();
    app.open_path(dir.join("a.txt"), false);
    app.run(&ctx, Cmd::ToggleSidebar);
    assert_eq!((app.sidebar.shown(), checked(&app)), (true, Some(true)));
    let folder = format!("Open “{}”", dir.file_name().unwrap().to_string_lossy());
    let nodes = labelled(&mut app, &ctx);
    let has = |l: &str| nodes.iter().any(|(n, _)| n == l);
    assert!(has("OPEN FILES") && has("a.txt") && has("Open Folder…") && has(&folder), "{nodes:?}");
    app.run(&ctx, Cmd::ToggleSidebar);
    assert_eq!((app.sidebar.shown(), checked(&app)), (false, Some(false)));
    assert!(!labelled(&mut app, &ctx).iter().any(|(n, _)| n == "Open Folder…"));
    // Back again, and its button opens the file's folder.
    app.run(&ctx, Cmd::ToggleSidebar);
    let at = labelled(&mut app, &ctx).into_iter().find(|(n, _)| *n == folder).unwrap().1.center();
    let b = |pressed| Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    frame(&mut app, &ctx, vec![Event::PointerMoved(at), b(true)]);
    frame(&mut app, &ctx, vec![b(false)]);
    assert_eq!(app.sidebar.roots, std::slice::from_ref(&dir));
    assert!(labelled(&mut app, &ctx).iter().any(|(n, _)| n == "a.txt"), "the tree");
    app.sidebar.close_folders();
    app.open_folder(dir.clone());
    assert_eq!(checked(&app), Some(true));
    app.run(&ctx, Cmd::ToggleSidebar);
    assert_eq!((app.sidebar.shown(), checked(&app)), (false, Some(false)));
    app.run(&ctx, Cmd::ToggleSidebar);
    assert_eq!((app.sidebar.shown(), checked(&app)), (true, Some(true)));
    std::fs::remove_dir_all(dir).ok();
}

/// Show Open Files lists even a single tab, and turning it on brings the
/// side bar out to show it.
#[test]
fn show_open_files_lists_one_tab_and_shows_the_side_bar() {
    let ctx = Context::default();
    let mut app = StrataApp::new(None, None);
    let dir = temp_dir("open-files");
    std::fs::write(dir.join("only.txt"), "x").unwrap();
    app.open_path(dir.join("only.txt"), false);
    app.open_folder(dir.clone());
    let listed = |app: &mut StrataApp| labelled(app, &ctx).iter().any(|(n, _)| n == "OPEN FILES");
    assert!(app.sidebar.show_open_files && listed(&mut app), "one tab is listed too");
    app.run(&ctx, Cmd::ToggleOpenFiles);
    assert!(!listed(&mut app));
    app.run(&ctx, Cmd::ToggleSidebar);
    assert!(!app.sidebar.shown());
    app.run(&ctx, Cmd::ToggleOpenFiles);
    assert!(app.sidebar.shown() && listed(&mut app), "turned on, it's shown");
    std::fs::remove_dir_all(dir).ok();
}

fn tab_rects(ctx: &Context) -> Vec<(usize, Rect)> {
    ctx.data(|d| d.get_temp::<Vec<(usize, Rect)>>(egui::Id::new("test-tab-rects"))).unwrap_or_default()
}

#[test]
fn the_tab_strip_follows_the_current_tab_and_the_wheel() {
    let ctx = Context::default();
    let mut app = StrataApp::new(None, None);
    for n in 1..30 {
        let mut d = Document::untitled();
        d.title = Some(format!("a_fairly_long_tab_name_{n}.rs"));
        app.push_doc(d);
    }
    let size = vec2(640.0, 400.0);
    let visible = |ctx: &Context, i: usize| {
        let r = tab_rects(ctx).into_iter().find(|(j, _)| *j == i).expect("tab drawn").1;
        r.left() >= -0.5 && r.right() <= 640.0 + 0.5
    };
    for _ in 0..4 {
        frame_sized(&mut app, &ctx, size, vec![]);
    }
    let last = app.docs.len() - 1;
    assert_eq!(app.active, last);
    assert!(visible(&ctx, last), "the newest (current) tab is scrolled into view");
    app.activate(0);
    for _ in 0..4 {
        frame_sized(&mut app, &ctx, size, vec![]);
    }
    assert!(visible(&ctx, 0) && !visible(&ctx, last), "activating the first tab scrolls back");
    // A vertical wheel over the strip scrolls it sideways.
    let before = tab_rects(&ctx)[0].1.left();
    let over = egui::pos2(300.0, 40.0);
    frame_sized(&mut app, &ctx, size, vec![Event::PointerMoved(over)]);
    frame_sized(&mut app, &ctx, size, vec![Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: vec2(0.0, -200.0), modifiers: Modifiers::NONE, phase: egui::TouchPhase::Move }]);
    for _ in 0..10 {
        frame_sized(&mut app, &ctx, size, vec![]);
    }
    assert!(tab_rects(&ctx)[0].1.left() < before - 50.0, "{before} → {}", tab_rects(&ctx)[0].1.left());
    // The overflow list is offered.
    let overflow = egui::Id::new(("tab-strip", 0usize, app.link.id("tabs")));
    assert!(ctx.data(|d| d.get_temp::<(Option<egui::Id>, bool, f32)>(overflow)).is_some_and(|(_, o, _)| o));
}

#[test]
fn completions_near_the_bottom_open_above_the_caret() {
    let ctx = Context::default();
    let mut app = StrataApp::new(None, None);
    let mut body: String = (0..60).map(|i| format!("let identifier_number_{i} = {i};\n")).collect();
    body += "ident";
    app.docs[0].editor.insert_text(&body);
    app.docs[0].view.reveal();
    for _ in 0..4 {
        frame(&mut app, &ctx, vec![]);
    }
    app.run(&ctx, Cmd::ShowCompletions);
    for _ in 0..3 {
        frame(&mut app, &ctx, vec![]);
    }
    let caret = app.docs[0].view.caret_rect().expect("caret on screen");
    let popup = ctx.memory(|m| m.area_rect(egui::Id::new("completions"))).expect("the list is up");
    assert!(popup.bottom() <= caret.top() + 0.5, "{popup:?} vs caret {caret:?}");
    // At the top of a file it opens below.
    let mut app = StrataApp::new(None, None);
    app.docs[0].editor.insert_text("identifier_one identifier_two\nide");
    let ctx = Context::default();
    for _ in 0..4 {
        frame(&mut app, &ctx, vec![]);
    }
    app.run(&ctx, Cmd::ShowCompletions);
    for _ in 0..3 {
        frame(&mut app, &ctx, vec![]);
    }
    let caret = app.docs[0].view.caret_rect().expect("caret on screen");
    let popup = ctx.memory(|m| m.area_rect(egui::Id::new("completions"))).expect("the list is up");
    assert!(popup.top() >= caret.bottom() - 0.5, "{popup:?} vs caret {caret:?}");
}

#[test]
fn the_status_bar_drops_items_rather_than_overlap() {
    let ctx = Context::default();
    let fit_at = |width: f32| {
        let mut fit = None;
        let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(width, 100.0))), ..Default::default() };
        ctx.run_ui(raw, |ui| {
            fit = Some(status_fit(ui, ("Line 120, Column 40", false), true, false, false, None, None, (Some("main"), Some("3 to review")), (Some("1,234 words"), None)));
        })
        .textures_delta
        .clear();
        fit.unwrap()
    };
    let wide = fit_at(1400.0);
    assert!(!wide.short_pos && wide.right.iter().all(|&r| r));
    assert!(wide.left[StatusLeft::Conflict as usize] && wide.left[StatusLeft::Branch as usize] && wide.left[StatusLeft::Review as usize] && wide.left[StatusLeft::Words as usize]);
    let mid = fit_at(760.0);
    assert!(mid.left[StatusLeft::Conflict as usize], "the warning outranks the word count");
    assert!(!mid.left[StatusLeft::Words as usize]);
    let narrow = fit_at(380.0);
    assert!(narrow.right[0] && !narrow.right[4], "the syntax stays, the theme menu goes first");
    assert!(narrow.left.iter().all(|&l| !l));
    let tiny = fit_at(200.0);
    assert!(tiny.short_pos && tiny.right == [true, false, false, false, false]);
}

/// Where the welcome hints were painted (their text's rects).
fn welcome_rects(out: &egui::FullOutput) -> Vec<Rect> {
    out.shapes
        .iter()
        .filter_map(|c| match &c.shape {
            egui::Shape::Text(t) if t.galley.text() == "Goto Anything" || t.galley.text() == "Strata" => Some(t.visual_bounding_rect().intersect(c.clip_rect)),
            _ => None,
        })
        .collect()
}

#[test]
fn welcome_hints_stay_in_the_editor_area() {
    let dir = temp_dir("welcome");
    let ctx = Context::default();
    let mut app = StrataApp::new(None, None);
    app.open_folder(dir.clone());
    // A tall bottom panel (the build output) takes most of the height.
    app.build.visible = true;
    let mut out = frame(&mut app, &ctx, vec![]);
    for _ in 0..3 {
        out = frame(&mut app, &ctx, vec![]);
    }
    let panes = ctx.data(|d| d.get_temp::<Rect>(egui::Id::new(("panes-rect", app.link.id("panes"))))).unwrap();
    let shown = welcome_rects(&out);
    assert!(!shown.is_empty(), "the hints show in a roomy window");
    for r in shown {
        assert!(r.is_negative() || panes.contains_rect(r), "{r:?} outside the editor area {panes:?}");
    }
    // A small editor area: no hints at all.
    let mut out = frame_sized(&mut app, &ctx, vec2(520.0, 340.0), vec![]);
    for _ in 0..3 {
        out = frame_sized(&mut app, &ctx, vec2(520.0, 340.0), vec![]);
    }
    assert!(welcome_rects(&out).is_empty());
    std::fs::remove_dir_all(dir).ok();
}

fn frame_fullscreen(app: &mut StrataApp, ctx: &Context, full: bool) -> Vec<egui::ViewportCommand> {
    let time = Some(ctx.input(|i| i.time) + 0.25);
    let mut raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 600.0))), time, ..Default::default() };
    raw.viewports.entry(egui::ViewportId::ROOT).or_default().fullscreen = Some(full);
    let mut out = ctx.run_ui(raw, |ui| {
        let mut frame = eframe::Frame::_new_kittest();
        eframe::App::ui(app, ui, &mut frame);
    });
    out.textures_delta.clear();
    out.viewport_output.remove(&egui::ViewportId::ROOT).map(|v| v.commands).unwrap_or_default()
}

#[test]
fn full_screen_follows_the_window() {
    use crate::menus::Item;
    fn checked(app: &StrataApp) -> Option<bool> {
        fn find(items: &[Item]) -> Option<Option<bool>> {
            items.iter().find_map(|it| match it {
                Item::Action { label, check, .. } if label == "Toggle Full Screen" => Some(*check),
                Item::Sub(_, kids) => find(kids),
                _ => None,
            })
        }
        app.menus().iter().find_map(|(_, items)| find(items)).flatten()
    }
    let ctx = Context::default();
    let mut app = StrataApp::new(None, None);
    frame_fullscreen(&mut app, &ctx, false);
    assert_eq!(checked(&app), Some(false));
    // Full screen from the window's own button: the menu sees it.
    frame_fullscreen(&mut app, &ctx, true);
    assert_eq!(checked(&app), Some(true));
    // Toggle Full Screen now leaves it (the old flag would have asked for full screen again).
    app.run(&ctx, Cmd::Fullscreen);
    let cmds = frame_fullscreen(&mut app, &ctx, true);
    assert!(cmds.contains(&egui::ViewportCommand::Fullscreen(false)), "{cmds:?}");
    // Distraction-free from full screen, and back: still full screen.
    frame_fullscreen(&mut app, &ctx, true);
    app.run(&ctx, Cmd::DistractionFree);
    frame_fullscreen(&mut app, &ctx, true);
    app.run(&ctx, Cmd::DistractionFree);
    let cmds = frame_fullscreen(&mut app, &ctx, true);
    assert!(cmds.contains(&egui::ViewportCommand::Fullscreen(true)) && !cmds.contains(&egui::ViewportCommand::Fullscreen(false)), "{cmds:?}");
    // From a normal window it ends in a normal window.
    frame_fullscreen(&mut app, &ctx, false);
    app.run(&ctx, Cmd::DistractionFree);
    frame_fullscreen(&mut app, &ctx, true);
    app.run(&ctx, Cmd::DistractionFree);
    let cmds = frame_fullscreen(&mut app, &ctx, true);
    assert!(cmds.contains(&egui::ViewportCommand::Fullscreen(false)), "{cmds:?}");
}

#[test]
fn trashed_files_close_their_tabs_but_keep_unsaved_text() {
    let dir = temp_dir("trash");
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    let (clean, dirty, other) = (dir.join("sub/clean.txt"), dir.join("sub/dirty.txt"), dir.join("other.txt"));
    for p in [&clean, &dirty, &other] {
        std::fs::write(p, "text\n").unwrap();
    }
    let mut app = StrataApp::new(Some(other.clone()), None);
    app.open_path(clean.clone(), false);
    app.open_path(dirty.clone(), false);
    app.doc().editor.insert_text("unsaved ");
    // As if Move to Trash took the folder.
    std::fs::remove_dir_all(dir.join("sub")).unwrap();
    app.follow_delete(&dir.join("sub"));
    assert!(!app.has_open_file(&clean), "a clean tab closes");
    let i = app.docs.iter().position(|d| d.path.as_ref() == Some(&dirty)).expect("the edited tab stays");
    assert!(app.docs[i].deleted && app.docs[i].is_dirty());
    assert!(app.has_open_file(&other));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn idle_frames_skip_menus_title_and_git_marks() {
    let ctx = Context::default();
    let mut app = StrataApp::new(None, None);
    let titles = |out: &egui::FullOutput| out.viewport_output.get(&egui::ViewportId::ROOT).map_or(0, |v| v.commands.iter().filter(|c| matches!(c, egui::ViewportCommand::Title(_))).count());
    let out = frame(&mut app, &ctx, vec![]);
    assert_eq!(titles(&out), 1, "the first frame names the window");
    app.menus_cache = None;
    app.fresh_menus(&ctx);
    let built = app.menus_cache.as_ref().map(|c| c.1);
    let out = frame(&mut app, &ctx, vec![]);
    assert_eq!(titles(&out), 0, "an unchanged title isn't sent again");
    assert!(!app.fresh_menus(&ctx), "nothing happened: the menus are kept");
    assert_eq!(app.menus_cache.as_ref().map(|c| c.1), built);
    // Typing changes things (the title gets a dot, Undo comes alive).
    frame(&mut app, &ctx, vec![Event::Text("x".into())]);
    frame(&mut app, &ctx, vec![press(Key::A, Modifiers::NONE)]);
    if cfg!(target_os = "macos") {
        // (Elsewhere the in-window menu bar already rebuilt them during that frame.)
        assert!(app.fresh_menus(&ctx), "input rebuilds them");
    }
    assert_ne!(app.menus_cache.as_ref().map(|c| c.1), built);
    // A second later they are rebuilt anyway (background changes): by the
    // in-window menu bar during a frame, or the next time macOS asks.
    let before = app.menus_cache.as_ref().map(|c| c.1);
    for _ in 0..5 {
        frame(&mut app, &ctx, vec![]);
    }
    app.fresh_menus(&ctx);
    assert_ne!(app.menus_cache.as_ref().map(|c| c.1), before);
    // Git marks: kept between frames, dropped when the gutter is turned off.
    app.docs[0].git_changes = vec![(0..1, crate::git::LineChange::Added)];
    frame(&mut app, &ctx, vec![]);
    assert_eq!(app.docs[0].view.line_marks.len(), 1);
    app.settings.git_gutter = false;
    frame(&mut app, &ctx, vec![]);
    assert!(app.docs[0].view.line_marks.is_empty());
}

#[test]
fn cjk_file_names_load_the_cjk_fonts() {
    let loaded = |ctx: &Context| ctx.data(|d| d.get_temp::<bool>(egui::Id::new("strata-cjk-fonts")).is_some());
    let dir = temp_dir("cjk");
    let file = dir.join("中文文件名.rs");
    std::fs::write(&file, "fn main() {}\n").unwrap();
    // ASCII names leave them unloaded (they're tens of MB).
    let ctx = Context::default();
    let mut app = StrataApp::new(None, None);
    frame(&mut app, &ctx, vec![]);
    assert!(!loaded(&ctx));
    // A tab named in Chinese (the editor text is ASCII).
    let ctx = Context::default();
    let mut app = StrataApp::new(Some(file.clone()), None);
    frame(&mut app, &ctx, vec![]);
    assert!(loaded(&ctx), "the tab name asked for them");
    // The side bar alone (no tab, no CJK text anywhere else).
    let ctx = Context::default();
    let mut app = StrataApp::new(None, None);
    app.open_folder(dir.clone());
    app.settings.show_tabs = false;
    frame(&mut app, &ctx, vec![]);
    frame(&mut app, &ctx, vec![]);
    assert!(loaded(&ctx), "the side bar row asked for them");
    // The status bar.
    let ctx = Context::default();
    let mut app = StrataApp::new(None, None);
    app.status = "Renamed to 名前.txt".into();
    frame(&mut app, &ctx, vec![]);
    assert!(loaded(&ctx), "the status message asked for them");
    std::fs::remove_dir_all(dir).ok();
}

/// Interface text is legible in every bundled scheme: body text and
/// secondary ("weak") text meet WCAG AA (4.5:1) on every surface they're
/// drawn on, high-contrast schemes keep AAA body text, and field
/// placeholders (drawn in the weak color) clear 3:1.
#[test]
fn interface_text_meets_wcag_aa_in_every_scheme() {
    use crate::highlight::themes::contrast;
    let mut failures = Vec::new();
    let names = crate::highlight::theme_names();
    assert!(names.len() > 20, "{names:?}");
    for name in names {
        let mut hl = crate::highlight::Highlighter::for_editor();
        hl.set_theme(name);
        let v = chrome_visuals(&hl);
        let (text, weak) = (v.text_color(), v.weak_text_color());
        let body_min = if hl.kind().is_high_contrast() { 7.0 } else { WCAG_AA };
        for (i, s) in chrome_surfaces(&v, hl.background()).into_iter().enumerate() {
            for (what, color, min) in [("text", text, body_min), ("secondary text", weak, WCAG_AA), ("placeholder", weak, 3.0)] {
                let c = contrast(color, s);
                if c < min {
                    failures.push(format!("{name}: {what} {color:?} on surface {i} {s:?}: {c:.2} < {min}"));
                }
            }
        }
        // Secondary text still reads as secondary next to body text.
        assert_ne!(text, weak, "{name}");
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A menu item by its path ("View", "Focus Group", "Group 2"): (check, enabled).
fn menu_item(app: &StrataApp, path: &[&str]) -> Option<(Option<bool>, bool)> {
    use crate::menus::Item;
    fn find(items: &[Item], path: &[&str]) -> Option<(Option<bool>, bool)> {
        items.iter().find_map(|it| match (it, path) {
            (Item::Action { label, check, enabled, .. }, [last]) if label == last => Some((*check, *enabled)),
            (Item::Sub(name, kids), [first, rest @ ..]) if name == first => find(kids, rest),
            _ => None,
        })
    }
    app.menus().iter().find_map(|(name, items)| (name == path[0]).then(|| find(items, &path[1..])).flatten())
}

/// With the status bar hidden (or distraction-free), status messages show
/// for a few seconds at the bottom of the window instead of going nowhere.
#[test]
fn status_messages_toast_while_the_status_bar_is_hidden() {
    let ctx = Context::default();
    let mut app = StrataApp::new(None, None);
    let shows = |app: &mut StrataApp, text: &str| shown_text(app, &ctx).iter().any(|n| n == text);
    app.run(&ctx, Cmd::ToggleStatusBar);
    assert!(!app.settings.show_status_bar);
    app.status = "Saved project x".into();
    assert!(shows(&mut app, "Saved project x"), "shown though the status bar is hidden");
    for _ in 0..20 {
        frame(&mut app, &ctx, vec![]);
    }
    assert!(!shows(&mut app, "Saved project x"), "and gone a few seconds later");
    // Distraction-free too.
    app.run(&ctx, Cmd::ToggleStatusBar);
    app.run(&ctx, Cmd::DistractionFree);
    app.status = "No macro recorded".into();
    assert!(shows(&mut app, "No macro recorded"));
}

/// Leaving full screen some other way (the window's button, Esc) ends
/// distraction-free mode, which needs it; its own switch to full screen
/// taking a few frames doesn't.
#[test]
fn leaving_full_screen_ends_distraction_free() {
    let ctx = Context::default();
    let mut app = StrataApp::new(None, None);
    app.sidebar.visible = true;
    frame_fullscreen(&mut app, &ctx, false);
    app.run(&ctx, Cmd::DistractionFree);
    frame_fullscreen(&mut app, &ctx, false);
    assert!(app.distraction.is_some(), "still on its way to full screen");
    frame_fullscreen(&mut app, &ctx, true);
    assert!(app.distraction.is_some());
    frame_fullscreen(&mut app, &ctx, false);
    assert!(app.distraction.is_none(), "left with the window");
    assert!(app.sidebar.visible, "the side bar is back");
    assert_eq!(menu_item(&app, &["View", "Toggle Distraction Free"]), Some((Some(false), true)));
}

/// In distraction-free mode panels aren't shown, so their check marks are
/// off; asking for one leaves the mode so it shows.
#[test]
fn panels_asked_for_in_distraction_free_show() {
    let ctx = Context::default();
    let mut app = StrataApp::new(None, None);
    app.settings.markdown_preview = true;
    let items: [(&[&str], Cmd); 6] = [
        (&["View", "Toggle Minimap"], Cmd::ToggleMinimap),
        (&["View", "Toggle Tabs"], Cmd::ToggleTabs),
        (&["View", "Toggle Status Bar"], Cmd::ToggleStatusBar),
        (&["View", "Show Panel", "Build Results"], Cmd::ToggleBuildResults),
        (&["View", "Markdown Preview"], Cmd::ToggleMarkdownPreview),
        (&["Tools", "AI", "Show Panel"], Cmd::AiTogglePanel),
    ];
    for (path, cmd) in items {
        app.run(&ctx, Cmd::DistractionFree);
        frame(&mut app, &ctx, vec![]);
        assert_eq!(menu_item(&app, path).map(|i| i.0), Some(Some(false)), "{path:?}: hidden, so not checked");
        app.run(&ctx, cmd);
        assert!(app.distraction.is_none(), "{path:?}: left distraction-free mode");
        assert_eq!(menu_item(&app, path).map(|i| i.0), Some(Some(true)), "{path:?}: shown");
        assert!(app.status.starts_with("Left Distraction Free"), "{}", app.status);
    }
    // Off again outside it.
    app.run(&ctx, Cmd::ToggleMinimap);
    assert!(!app.settings.minimap);
}

/// Turning the minimap on in a pane too narrow for it says why it doesn't show.
#[test]
fn minimap_in_a_narrow_pane_says_why_it_is_hidden() {
    let ctx = Context::default();
    let mut app = StrataApp::new(None, None);
    app.settings.minimap = false;
    frame_sized(&mut app, &ctx, vec2(500.0, 400.0), vec![]);
    app.run(&ctx, Cmd::ToggleMinimap);
    assert!(app.settings.minimap);
    assert!(app.status.contains("wider than 640"), "{}", app.status);
    app.run(&ctx, Cmd::ToggleMinimap);
    frame(&mut app, &ctx, vec![]);
    app.status.clear();
    app.run(&ctx, Cmd::ToggleMinimap);
    assert!(app.status.is_empty(), "wide enough: nothing to say");
}

/// While a macro records, the status bar says so, and the palette offers to stop.
#[test]
fn recording_a_macro_shows_in_the_status_bar_and_palette() {
    let ctx = Context::default();
    let mut app = StrataApp::new(None, None);
    let rec = |app: &mut StrataApp| shown_text(app, &ctx).iter().any(|n| n == "● REC");
    assert!(!rec(&mut app));
    app.run(&ctx, Cmd::RecordMacro);
    app.status = "Something else".into();
    assert!(rec(&mut app), "lasts past the status message");
    assert_eq!(menu_item(&app, &["Tools", "Stop Recording Macro"]).map(|i| i.1), Some(true));
    app.open_palette(&ctx, crate::palette::Mode::Commands, "recording macro");
    frame(&mut app, &ctx, vec![]);
    assert_eq!(app.palette.selected_label(), Some("Tools: Stop Recording Macro"));
    frame(&mut app, &ctx, vec![press(Key::Escape, Modifiers::NONE)]);
    app.run(&ctx, Cmd::RecordMacro);
    assert!(!rec(&mut app));
}

/// View ▸ Focus Group offers only the groups there are.
#[test]
fn focus_group_items_need_the_group() {
    let ctx = Context::default();
    let mut app = StrataApp::new(None, None);
    let enabled = |app: &StrataApp, g: &str| menu_item(app, &["View", "Focus Group", g]).map(|i| i.1);
    assert_eq!((enabled(&app, "Group 1"), enabled(&app, "Group 2")), (Some(true), Some(false)));
    app.run(&ctx, Cmd::LayoutColumns2);
    assert_eq!((enabled(&app, "Group 2"), enabled(&app, "Group 3")), (Some(true), Some(false)));
    assert_eq!(menu_item(&app, &["View", "Move File to Group", "Group 4"]).map(|i| i.1), Some(true), "moving makes the group");
}

/// Ruler, Word Wrap Column and the whitespace items check what the current
/// file gets, and say so when `.editorconfig` or the syntax's settings
/// keep a choice from applying to it.
#[test]
fn view_settings_checks_follow_the_file() {
    let dir = temp_dir("effective");
    std::fs::write(dir.join(".editorconfig"), "root = true\n[*.rs]\nmax_line_length = 100\n").unwrap();
    std::fs::write(dir.join("a.rs"), "fn main() {}\n").unwrap();
    let ctx = Context::default();
    let mut app = StrataApp::new(None, None);
    app.settings.rulers = vec![];
    app.settings.syntax_settings.insert("Rust".into(), crate::doc_settings::Layer { draw_white_space: Some("all".into()), ..Default::default() });
    app.open_path(dir.join("a.rs"), false);
    frame(&mut app, &ctx, vec![]);
    let check = |app: &StrataApp, path: &[&str]| menu_item(app, path).and_then(|i| i.0);
    assert_eq!(check(&app, &["View", "Ruler", "100"]), Some(true));
    assert_eq!(check(&app, &["View", "Ruler", "None"]), Some(false));
    assert_eq!(check(&app, &["View", "Word Wrap Column", "100"]), Some(true));
    assert_eq!(check(&app, &["View", "Editor Extras", "Show All Whitespace"]), Some(true));
    assert_eq!(check(&app, &["View", "Editor Extras", "Show Whitespace in Selection"]), Some(false));
    app.run_act(&ctx, crate::menus::Act::Named("ruler:80".into()));
    assert_eq!(app.settings.rulers, vec![80]);
    assert_eq!(app.status, "Saved, but .editorconfig sets the ruler for a.rs");
    app.run_act(&ctx, crate::menus::Act::Named("x:ws_none".into()));
    assert_eq!(app.status, "Saved, but syntax_settings (Rust) sets the whitespace for a.rs");
    // A choice that applies says nothing.
    app.status.clear();
    app.run_act(&ctx, crate::menus::Act::Named("x:trailing".into()));
    assert!(app.status.is_empty(), "{}", app.status);
    // When settings.json can't be written, that's what it says.
    let not_a_folder = dir.join("not-a-folder");
    std::fs::write(&not_a_folder, "").unwrap();
    app.config = Some(not_a_folder);
    for act in ["ruler:90", "wrapcol:90", "x:ws_all"] {
        app.run_act(&ctx, crate::menus::Act::Named(act.into()));
        assert!(app.status.starts_with("Could not write settings"), "{act}: {}", app.status);
    }
    std::fs::remove_dir_all(dir).ok();
}
