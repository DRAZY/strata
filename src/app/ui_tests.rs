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
    frame(&mut app, &ctx, vec![press(Key::W, primary())]);
    assert_eq!(app.docs.len(), 2, "Cmd+W didn't close the tab behind the dialog");
    frame(&mut app, &ctx, vec![press(Key::P, primary() | Modifiers::SHIFT)]);
    assert!(!app.palette.open, "nor did Cmd+Shift+P open the palette");
    frame(&mut app, &ctx, vec![press(Key::F, primary())]);
    assert!(!app.find.open);
    // Typing goes to the name field, not the editor.
    frame(&mut app, &ctx, vec![Event::Text("x".into())]);
    assert_eq!(app.sidebar.prompt_state().map(|(n, _)| n.clone()).as_deref(), Some("b.txtx"));
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
    app.run(&ctx, Cmd::ToggleSidebar);
    assert!(!app.sidebar.shown() && app.status.contains("folder"), "{}", app.status);
    let dir = temp_dir("toggle");
    app.open_folder(dir.clone());
    assert_eq!(checked(&app), Some(true));
    app.run(&ctx, Cmd::ToggleSidebar);
    assert_eq!((app.sidebar.shown(), checked(&app)), (false, Some(false)));
    app.run(&ctx, Cmd::ToggleSidebar);
    assert_eq!((app.sidebar.shown(), checked(&app)), (true, Some(true)));
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
        let _ = ctx.run_ui(raw, |ui| {
            fit = Some(status_fit(ui, "Line 120, Column 40", true, false, false, Some("main"), Some("1,234 words")));
        });
        fit.unwrap()
    };
    let wide = fit_at(1400.0);
    assert!(!wide.short_pos && wide.right.iter().all(|&r| r));
    assert!(wide.left[StatusLeft::Conflict as usize] && wide.left[StatusLeft::Branch as usize] && wide.left[StatusLeft::Words as usize]);
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
