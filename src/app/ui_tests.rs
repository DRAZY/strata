//! Whole-frame checks of window behavior: dialogs and the keyboard, focus,
//! the tab strip, the side bar.

use super::*;
use egui::{Context, Event, Key, Modifiers, Pos2, RawInput};

pub(super) fn frame_sized(app: &mut StrataApp, ctx: &Context, size: egui::Vec2, events: Vec<Event>) -> egui::FullOutput {
    let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)), events, ..Default::default() };
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
