//! Commands that used to do nothing, or say the wrong thing, when there was
//! nothing for them to act on: each now says why (or does the obvious thing).

use super::ui_tests::{frame, press, temp_dir};
use super::*;
use egui::{Context, Key, Modifiers};

fn app_with(text: &str) -> StrataApp {
    let mut app = StrataApp::new(None, None);
    app.docs[0].editor.insert_text(text);
    app
}

/// Every text drawn in a frame.
fn drawn(out: &egui::FullOutput) -> Vec<String> {
    fn walk(shape: &egui::Shape, acc: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(t) => acc.push(t.galley.text().to_owned()),
            egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, acc)),
            _ => {}
        }
    }
    let mut acc = Vec::new();
    out.shapes.iter().for_each(|c| walk(&c.shape, &mut acc));
    acc
}

fn selected(app: &StrataApp) -> String {
    let e = &app.docs[app.active].editor;
    e.buffer.slice(e.primary().range())
}

#[test]
fn find_next_with_no_matches_says_so() {
    let ctx = Context::default();
    let mut app = app_with("cat dog\n");
    app.find.query = Query::literal("bird", false);
    app.run(&ctx, Cmd::FindNext);
    assert_eq!(app.status, "No matches for “bird”");
    app.status.clear();
    app.run(&ctx, Cmd::FindPrev);
    assert_eq!(app.status, "No matches for “bird”");
}

#[test]
fn goto_anything_without_a_folder_offers_recent_files() {
    let dir = temp_dir("goto-recent");
    let recent = dir.join("recent-notes.txt");
    std::fs::write(&recent, "x").unwrap();
    let ctx = Context::default();
    let mut app = StrataApp::new(None, None);
    app.remember_file(&recent);
    app.run(&ctx, Cmd::GotoAnything);
    frame(&mut app, &ctx, vec![]);
    let texts = drawn(&frame(&mut app, &ctx, vec![]));
    assert!(texts.iter().any(|t| t == "Open a folder to search its files"), "{texts:?}");
    assert_eq!(app.palette.selected_label(), Some(recent.display().to_string().as_str()));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn backlinks_outside_a_vault_explain_and_keep_the_setting() {
    let ctx = Context::default();
    let mut app = app_with("# Not a note\n");
    let before = app.settings.notes_backlinks.clone();
    app.run(&ctx, Cmd::NotesToggleBacklinks);
    assert_eq!(app.settings.notes_backlinks, before, "the setting is left alone");
    assert!(app.status.contains("notes folder"), "{}", app.status);
}

#[test]
fn quick_find_with_a_caret_uses_the_word_under_it() {
    let ctx = Context::default();
    let mut app = app_with("foo bar foo");
    app.doc().editor.select_ranges(&[1..1], 0);
    app.run(&ctx, Cmd::QuickFind);
    assert_eq!(app.doc().editor.primary().range(), 8..11);
}

#[test]
fn use_selection_for_find_takes_the_word_and_says_so() {
    let ctx = Context::default();
    let mut app = app_with("foo bar");
    app.find.query.text = "earlier".into();
    app.doc().editor.select_ranges(&[5..5], 0);
    app.run(&ctx, Cmd::UseSelectionForFind);
    assert_eq!(app.find.query.text, "bar");
    assert_eq!(app.status, "Find: “bar”");
    app.run(&ctx, Cmd::UseSelectionForReplace);
    assert_eq!(app.find.replacement, "bar");
    assert_eq!(app.status, "Replace with: “bar”");
    // Nothing under the caret: the query stays.
    let mut app = app_with("foo  bar");
    app.find.query.text = "earlier".into();
    app.doc().editor.select_ranges(&[4..4], 0);
    app.run(&ctx, Cmd::UseSelectionForFind);
    assert_eq!(app.find.query.text, "earlier");
    assert!(!app.status.is_empty());
}

#[test]
fn build_with_no_build_system_says_so_in_the_panel() {
    let ctx = Context::default();
    let mut app = app_with("plain text\n");
    app.run(&ctx, Cmd::Build);
    assert!(app.build.visible, "the Build Results panel shows the message");
    assert!(app.build.output_text().contains("New Build System"), "{}", app.build.output_text());
    app.build.visible = false;
    app.run(&ctx, Cmd::BuildWith);
    assert!(!app.palette.open, "no empty picker");
    assert!(app.status.starts_with("No build system for this file"), "{}", app.status);
    assert!(app.build.visible);
}

#[test]
fn next_modification_says_why_there_are_none() {
    let ctx = Context::default();
    let mut app = app_with("untitled\n");
    app.run(&ctx, Cmd::NextModification);
    assert_eq!(app.status, "Save the file first");
    app.status.clear();
    app.run(&ctx, Cmd::NextDifference);
    assert!(app.status.starts_with("Save the file first"), "{}", app.status);

    let dir = temp_dir("no-git");
    let path = dir.join("a.txt");
    std::fs::write(&path, "a\n").unwrap();
    app.open_path(path.clone(), false);
    app.git.request(&path, ropey::Rope::from_str("a\n"), 0);
    let start = std::time::Instant::now();
    while app.git.poll().is_none() && start.elapsed().as_secs() < 10 {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    app.run(&ctx, Cmd::PrevModification);
    assert_eq!(app.status, "Not in a Git repository");
    app.status.clear();
    app.run(&ctx, Cmd::PrevDifference);
    assert!(app.status.starts_with("Not in a Git repository"), "{}", app.status);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn next_build_result_before_any_build_says_so() {
    let ctx = Context::default();
    let mut app = app_with("");
    app.run(&ctx, Cmd::NextResult);
    assert_eq!(app.status, "No build results");
    app.status.clear();
    app.run(&ctx, Cmd::PrevResult);
    assert_eq!(app.status, "No build results");
}

#[test]
fn show_problems_without_a_language_server_says_so() {
    let ctx = Context::default();
    let mut app = app_with("text\n");
    app.settings.lsp_enabled = true;
    app.run(&ctx, Cmd::LspProblems);
    assert_eq!(app.status, "Problems: no language server for this file");
}

/// The palette and shortcuts run what the menu greys out: they refuse, and
/// say why, instead of doing something odd or nothing.
#[test]
fn commands_the_menu_disables_refuse_with_a_reason() {
    let ctx = Context::default();
    let mut app = app_with("plain text\n");
    for (cmd, why) in [
        (Cmd::RevertFile, "never saved"),
        (Cmd::CloseGroup, "one group"),
        (Cmd::SwitchHeader, "Not a C"),
        (Cmd::NewBuildSystem, "settings folder"),
        (Cmd::ExportMarkdownHtml, "Markdown"),
    ] {
        app.status.clear();
        app.run(&ctx, cmd);
        assert!(app.status.contains(why), "{cmd:?}: {:?}", app.status);
        assert_eq!(app.unavailable(cmd).is_some(), cmd != Cmd::SwitchHeader, "{cmd:?}");
    }
}

#[test]
fn clear_recent_items_keeps_reopen_closed_file() {
    let ctx = Context::default();
    let mut app = StrataApp::new(None, None);
    app.remember_file(Path::new("/f.txt"));
    app.closed.push(PathBuf::from("/closed.txt"));
    app.run(&ctx, Cmd::ClearRecent);
    assert!(app.recent_files.is_empty());
    assert_eq!(app.closed, vec![PathBuf::from("/closed.txt")]);
}

#[test]
fn show_completions_with_none_says_so() {
    let ctx = Context::default();
    let mut app = app_with("");
    app.run(&ctx, Cmd::ShowCompletions);
    assert_eq!(app.status, "No completions");
}

/// Incremental Find: Return keeps the match the typing found and goes back
/// to the editor. Find's Return moves on to the next match.
#[test]
fn incremental_find_return_closes_on_the_match() {
    let ctx = Context::default();
    let mut app = app_with("one two one two\n");
    app.doc().editor.select_ranges(&[0..0], 0);
    for _ in 0..2 {
        frame(&mut app, &ctx, vec![]);
    }
    app.run(&ctx, Cmd::IncrementalFind);
    for _ in 0..3 {
        frame(&mut app, &ctx, vec![]);
    }
    frame(&mut app, &ctx, vec![egui::Event::Text("two".into())]);
    frame(&mut app, &ctx, vec![]);
    assert_eq!((app.find.query.text.as_str(), app.docs[0].editor.primary().range()), ("two", 4..7));
    frame(&mut app, &ctx, vec![press(Key::Enter, Modifiers::NONE)]);
    assert!(!app.find.open, "Return closed the bar");
    assert_eq!((app.doc().editor.primary().range(), selected(&app)), (4..7, "two".to_owned()), "on the match typing found");

    // Find keeps the bar and steps.
    app.doc().editor.select_ranges(&[4..7], 0);
    app.run(&ctx, Cmd::Find);
    frame(&mut app, &ctx, vec![]);
    frame(&mut app, &ctx, vec![]);
    frame(&mut app, &ctx, vec![press(Key::Enter, Modifiers::NONE)]);
    assert!(app.find.open);
    assert_eq!(app.doc().editor.primary().range(), 12..15);
}

#[test]
fn find_in_files_has_one_name() {
    assert_eq!(Cmd::FindInFiles.name(), "Find: Find in Files…");
}
