//! Notes mode driven through whole frames: `[[` completion, ⌘/Ctrl-click,
//! creating a missing note, backlinks after an outside change, and a rename
//! whose link updates are undone byte for byte.

use super::ui_tests::{frame, press, temp_dir};
use super::*;
use egui::{Context, Event, Modifiers, PointerButton, RawInput};

fn write(dir: &Path, rel: &str, text: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn text(app: &StrataApp) -> String {
    let b = &app.docs[app.active].editor.buffer;
    b.slice(0..b.len_chars())
}

fn primary() -> Modifiers {
    if cfg!(target_os = "macos") { Modifiers::MAC_CMD | Modifiers::COMMAND } else { Modifiers::CTRL | Modifiers::COMMAND }
}

/// A frame with these modifiers held (they stay until changed again).
fn frame_mods(app: &mut StrataApp, ctx: &Context, events: Vec<Event>, modifiers: Modifiers) -> egui::FullOutput {
    let time = Some(ctx.input(|i| i.time) + 0.25);
    let events = std::iter::once(Event::ModifiersChanged(modifiers)).chain(events).collect();
    let raw = RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(1000.0, 600.0))), events, time, ..Default::default() };
    let mut out = ctx.run_ui(raw, |ui| {
        let mut f = eframe::Frame::_new_kittest();
        eframe::App::ui(app, ui, &mut f);
    });
    out.textures_delta.clear();
    out
}

/// Open `dir` and `note` in it; wait for the vault to be indexed.
fn open_vault(dir: &Path, note: &str, ctx: &Context) -> StrataApp {
    let mut app = StrataApp::new(Some(dir.to_owned()), None);
    app.open_path(dir.join(note), false);
    settle(&mut app, ctx);
    app
}

fn settle(app: &mut StrataApp, ctx: &Context) {
    for _ in 0..2 {
        frame(app, ctx, vec![]);
        app.notes.index.wait();
    }
    frame(app, ctx, vec![]);
}

/// Every text drawn this frame.
fn texts(out: &egui::FullOutput) -> Vec<String> {
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

#[test]
fn typing_double_brackets_completes_note_names_and_headings() {
    let dir = temp_dir("notes-complete");
    write(&dir, "Home.md", "");
    write(&dir, "Folder/Beta Note.md", "---\naliases: [Second]\n---\nbeta\n");
    write(&dir, "Alpha.md", "# Alpha\n## Section One\ntext ^blk-1\n");
    let ctx = Context::default();
    let mut app = open_vault(&dir, "Home.md", &ctx);
    for t in ["[", "[", "bet"] {
        frame(&mut app, &ctx, vec![Event::Text(t.into())]);
    }
    assert_eq!(text(&app), "[[bet]]", "brackets pair as typed");
    let c = app.notes.complete.as_ref().expect("the list is up");
    assert_eq!(c.items[0].label, "Beta Note");
    assert_eq!(c.items[0].detail, "Folder");
    assert!(!app.completion.is_open(), "word completions wait");
    let out = frame(&mut app, &ctx, vec![]);
    assert!(texts(&out).iter().any(|t| t == "Beta Note"), "the list is drawn");
    frame(&mut app, &ctx, vec![press(Key::Enter, Modifiers::NONE)]);
    assert_eq!(text(&app), "[[Beta Note]]", "the name is unique, so no folder");
    let d = &app.docs[app.active];
    assert_eq!(d.editor.primary().head, d.editor.buffer.len_chars(), "the cursor ends after the link");
    assert!(!app.notes_completing());

    // A heading of another note, then an alias.
    frame(&mut app, &ctx, vec![press(Key::Enter, Modifiers::NONE)]);
    for t in ["[", "[", "alpha#sec"] {
        frame(&mut app, &ctx, vec![Event::Text(t.into())]);
    }
    assert_eq!(app.notes.complete.as_ref().unwrap().items[0].label, "Section One");
    frame(&mut app, &ctx, vec![press(Key::Tab, Modifiers::NONE)]);
    assert!(text(&app).ends_with("\n[[Alpha#Section One]]"), "{:?}", text(&app));
    frame(&mut app, &ctx, vec![press(Key::Enter, Modifiers::NONE)]);
    for t in ["[", "[", "Secon"] {
        frame(&mut app, &ctx, vec![Event::Text(t.into())]);
    }
    let items = &app.notes.complete.as_ref().unwrap().items;
    let k = items.iter().position(|i| i.label == "Second").expect("aliases are offered");
    for _ in 0..k {
        frame(&mut app, &ctx, vec![press(Key::ArrowDown, Modifiers::NONE)]);
    }
    frame(&mut app, &ctx, vec![press(Key::Enter, Modifiers::NONE)]);
    assert!(text(&app).ends_with("\n[[Beta Note|Second]]"), "{:?}", text(&app));
    // Esc closes the list and the text stays as typed.
    frame(&mut app, &ctx, vec![press(Key::Enter, Modifiers::NONE)]);
    for t in ["[", "[", "al"] {
        frame(&mut app, &ctx, vec![Event::Text(t.into())]);
    }
    frame(&mut app, &ctx, vec![press(Key::Escape, Modifiers::NONE)]);
    assert!(!app.notes_completing());
    assert!(text(&app).ends_with("\n[[al]]"));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn command_click_on_a_link_opens_the_note_at_its_heading() {
    let dir = temp_dir("notes-click");
    write(&dir, "Home.md", "Go to [[Target#Part two]] now\n");
    write(&dir, "Target.md", "# Target\n\nintro\n\n## Part two\nhere\n");
    let ctx = Context::default();
    let mut app = open_vault(&dir, "Home.md", &ctx);
    // The link is marked as found.
    let line = app.docs[app.active].editor.buffer.line_text(0);
    let marks = app.docs[app.active].view.note_marks.for_line(0, &line).to_vec();
    assert_eq!(marks, [(6..25, crate::notes::marks::Mark::Link)]);
    // Where the link is on screen: put the caret inside it and read the caret's rect.
    app.docs[app.active].editor.select_ranges(&[10..10], 0);
    frame(&mut app, &ctx, vec![]);
    let at = app.docs[app.active].view.caret_rect().expect("caret drawn").center() + vec2(2.0, 0.0);
    app.docs[app.active].editor.select_ranges(&[0..0], 0);
    frame(&mut app, &ctx, vec![]);
    let button = |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: primary() };
    // A plain click only moves the caret.
    let plain = |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    frame(&mut app, &ctx, vec![Event::PointerMoved(at), plain(true)]);
    frame(&mut app, &ctx, vec![plain(false)]);
    assert!(app.docs[app.active].path.as_ref().unwrap().ends_with("Home.md"));
    frame_mods(&mut app, &ctx, vec![Event::PointerMoved(at), button(true)], primary());
    frame_mods(&mut app, &ctx, vec![button(false)], primary());
    frame_mods(&mut app, &ctx, vec![], Modifiers::NONE);
    let d = &app.docs[app.active];
    assert!(d.path.as_ref().unwrap().ends_with("Target.md"), "{:?}", d.path);
    assert_eq!(d.editor.buffer.line_of(d.editor.primary().head), 4, "at the heading");
    assert_eq!(app.docs.iter().filter(|d| d.path.is_some()).count(), 2);
    assert_eq!(d.editor.selections().len(), 1, "no extra cursor was added");
    // Jump Back returns to the link.
    app.run(&ctx, Cmd::JumpBack);
    assert!(app.docs[app.active].path.as_ref().unwrap().ends_with("Home.md"));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn following_a_link_to_a_missing_note_offers_to_create_it() {
    let dir = temp_dir("notes-create");
    write(&dir, "Home.md", "See [[Brand New Idea]].\n");
    write(&dir, ".obsidian/app.json", r#"{"newFileLocation":"folder","newFileFolderPath":"Inbox"}"#);
    let ctx = Context::default();
    let mut app = open_vault(&dir, "Home.md", &ctx);
    let line = app.docs[app.active].editor.buffer.line_text(0);
    assert_eq!(app.docs[app.active].view.note_marks.for_line(0, &line)[0].1, crate::notes::marks::Mark::Broken);
    app.docs[app.active].editor.select_ranges(&[8..8], 0);
    app.run(&ctx, Cmd::NotesFollowLink);
    assert!(matches!(app.notes.dialog, Some(super::notes_panel::Dialog::Create { .. })));
    // (A dialog is sized on its first frame and drawn from the next.)
    frame(&mut app, &ctx, vec![]);
    let out = frame(&mut app, &ctx, vec![]);
    assert!(texts(&out).iter().any(|t| t.contains("Create “Brand New Idea”?")));
    // Cancel leaves everything as it was.
    frame(&mut app, &ctx, vec![press(Key::Escape, Modifiers::NONE)]);
    assert!(app.notes.dialog.is_none() && !dir.join("Inbox").exists());
    app.run(&ctx, Cmd::NotesFollowLink);
    frame(&mut app, &ctx, vec![]);
    frame(&mut app, &ctx, vec![press(Key::Enter, Modifiers::NONE)]);
    let made = dir.join("Inbox").join("Brand New Idea.md");
    assert_eq!(std::fs::read_to_string(&made).unwrap(), "", "empty, in Obsidian's new-note folder");
    assert_eq!(app.docs[app.active].path.as_ref(), Some(&made));
    // Back in Home the link is found now.
    app.run(&ctx, Cmd::JumpBack);
    settle(&mut app, &ctx);
    let line = app.docs[app.active].editor.buffer.line_text(0);
    assert_eq!(app.docs[app.active].view.note_marks.for_line(0, &line)[0].1, crate::notes::marks::Mark::Link);
    // A name that can't be a file is refused, and nothing is written.
    write(&dir, "Bad.md", "[[what?]]\n");
    app.open_path(dir.join("Bad.md"), false);
    settle(&mut app, &ctx);
    app.docs[app.active].editor.select_ranges(&[3..3], 0);
    app.run(&ctx, Cmd::NotesFollowLink);
    assert!(app.notes.dialog.is_none() && app.status.contains("can't contain"), "{}", app.status);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn backlinks_follow_changes_made_outside_strata() {
    let dir = temp_dir("notes-backlinks");
    write(&dir, "A.md", "# A\n");
    write(&dir, "B.md", "Links to [[A]] here\n");
    let ctx = Context::default();
    let mut app = open_vault(&dir, "A.md", &ctx);
    assert!(app.backlinks_shown(), "a folder whose notes use wikilinks");
    let out = frame(&mut app, &ctx, vec![]);
    assert!(texts(&out).iter().any(|t| t == "B"), "B is listed");
    assert!(texts(&out).iter().any(|t| t.contains("Links to [[A]] here")), "with its line");
    // Obsidian (or a sync tool) writes a new note that links here.
    write(&dir, "Sub/C.md", "first line\nAnother [[A]] link, from C\n");
    // The periodic rescan picks it up within a few seconds of input time.
    let mut seen = false;
    for _ in 0..40 {
        frame(&mut app, &ctx, vec![]);
        app.notes.index.wait();
        let out = frame(&mut app, &ctx, vec![]);
        if texts(&out).iter().any(|t| t.contains("Another [[A]] link, from C")) {
            seen = true;
            break;
        }
    }
    assert!(seen, "the new backlink shows up");
    let v = app.notes.index.vault_for(&dir.join("A.md")).unwrap().clone();
    let a = v.find(&dir.join("A.md")).unwrap();
    assert_eq!(v.backlinks(a).len(), 2);
    // Clicking the line opens C there.
    let out = frame(&mut app, &ctx, vec![]);
    let _ = out;
    app.open_at(dir.join("Sub").join("C.md"), 1, 0..0);
    assert_eq!(app.docs[app.active].editor.buffer.line_of(app.docs[app.active].editor.primary().head), 1);
    // Hiding the panel is remembered as a setting.
    app.run(&ctx, Cmd::NotesToggleBacklinks);
    app.open_path(dir.join("A.md"), false);
    assert!(!app.backlinks_shown());
    assert_eq!(app.settings.notes_backlinks, "never");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn rename_updates_links_and_undo_restores_them_byte_for_byte() {
    let dir = temp_dir("notes-rename");
    let a_bytes = b"Two links: [[Old]] and [[Old#H|shown]]\r\nNot this: `[[Old]]`\r\n![[Old]] [md](Old.md#H)\r\n".to_vec();
    std::fs::write(dir.join("A.md"), &a_bytes).unwrap();
    write(&dir, "Old.md", "# H\nself [[Old#H]]\n");
    write(&dir, "B.md", "open in a tab: [[old]]\n");
    write(&dir, "C.md", "```\n[[Old]]\n```\nmentions Old only\n");
    let c_before = std::fs::read(dir.join("C.md")).unwrap();
    let ctx = Context::default();
    let mut app = open_vault(&dir, "B.md", &ctx);
    app.open_path(dir.join("Old.md"), false);
    settle(&mut app, &ctx);
    app.run(&ctx, Cmd::NotesRename);
    match app.notes.dialog.as_mut() {
        Some(super::notes_panel::Dialog::Name { name, .. }) => *name = "New Name".into(),
        _ => panic!("the name dialog is up"),
    }
    frame(&mut app, &ctx, vec![]);
    frame(&mut app, &ctx, vec![press(Key::Enter, Modifiers::NONE)]);
    assert!(dir.join("New Name.md").exists() && !dir.join("Old.md").exists());
    assert!(app.docs[app.active].path.as_ref().unwrap().ends_with("New Name.md"), "the tab follows");
    // The update is planned once the index is fresh, and every change is shown.
    for _ in 0..4 {
        app.notes.index.wait();
        frame(&mut app, &ctx, vec![]);
    }
    let out = frame(&mut app, &ctx, vec![]);
    let shown = texts(&out);
    assert!(shown.iter().any(|t| t.contains("Update 6 links in 3 notes")), "{shown:?}");
    assert!(shown.iter().any(|t| t.contains("Two links: [[New Name]] and [[New Name#H|shown]]")));
    frame(&mut app, &ctx, vec![press(Key::Enter, Modifiers::NONE)]);
    app.notes.disk.wait_until_done();
    frame(&mut app, &ctx, vec![]);
    assert!(app.status.starts_with("Updated 6 links in 3 notes (2 notes in open tabs, unsaved)"), "{}", app.status);
    assert_eq!(
        std::fs::read(dir.join("A.md")).unwrap(),
        b"Two links: [[New Name]] and [[New Name#H|shown]]\r\nNot this: `[[Old]]`\r\n![[New Name]] [md](New%20Name.md#H)\r\n".to_vec(),
        "only real links change; CRLF kept"
    );
    assert_eq!(std::fs::read(dir.join("C.md")).unwrap(), c_before, "code and plain mentions are left alone");
    let b = app.docs.iter().find(|d| d.path.as_ref().is_some_and(|p| p.ends_with("B.md"))).unwrap();
    assert_eq!(b.editor.buffer.slice(0..b.editor.buffer.len_chars()), "open in a tab: [[New Name]]\n");
    assert!(b.is_dirty(), "open tabs change unsaved");
    assert_eq!(text(&app), "# H\nself [[New Name#H]]\n", "the note's link to itself");

    app.run(&ctx, Cmd::NotesUndoRename);
    app.notes.disk.wait_until_done();
    frame(&mut app, &ctx, vec![]);
    assert_eq!(std::fs::read(dir.join("A.md")).unwrap(), a_bytes, "byte for byte");
    assert!(dir.join("Old.md").exists() && !dir.join("New Name.md").exists(), "the old name is back");
    let b = app.docs.iter().find(|d| d.path.as_ref().is_some_and(|p| p.ends_with("B.md"))).unwrap();
    assert_eq!(b.editor.buffer.slice(0..b.editor.buffer.len_chars()), "open in a tab: [[old]]\n");
    assert!(!b.is_dirty());
    let old = app.docs.iter().find(|d| d.path.as_ref().is_some_and(|p| p.ends_with("Old.md"))).expect("the tab follows back");
    assert_eq!(old.editor.buffer.slice(0..old.editor.buffer.len_chars()), "# H\nself [[Old#H]]\n");
    assert!(app.status.contains("back to"), "{}", app.status);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn sidebar_rename_offers_the_same_link_update() {
    let dir = temp_dir("notes-sidebar-rename");
    write(&dir, "A.md", "[[Old]]\n");
    write(&dir, "Old.md", "x\n");
    let ctx = Context::default();
    let mut app = open_vault(&dir, "A.md", &ctx);
    app.sidebar.ask_rename(&dir.join("Old.md"));
    if let Some((name, _)) = app.sidebar.prompt_state() {
        *name = "Renamed.md".into();
    }
    frame(&mut app, &ctx, vec![]);
    frame(&mut app, &ctx, vec![press(Key::Enter, Modifiers::NONE)]);
    assert!(dir.join("Renamed.md").exists());
    for _ in 0..4 {
        app.notes.index.wait();
        frame(&mut app, &ctx, vec![]);
    }
    assert!(matches!(&app.notes.dialog, Some(super::notes_panel::Dialog::Links(p)) if !p.waiting && p.files.len() == 1));
    // Don't Update leaves the link alone.
    frame(&mut app, &ctx, vec![press(Key::Escape, Modifiers::NONE)]);
    assert!(app.notes.dialog.is_none());
    assert_eq!(text(&app), "[[Old]]\n");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn daily_note_and_templates() {
    let dir = temp_dir("notes-daily");
    write(&dir, "Home.md", "");
    write(&dir, "Templates/Meeting.md", "# {{title}}\nDate: {{date:YYYY}}\n");
    let ctx = Context::default();
    let mut app = open_vault(&dir, "Home.md", &ctx);
    app.run(&ctx, Cmd::NotesDailyNote);
    let today = crate::notes::dates::format("YYYY-MM-DD", &crate::notes::dates::DateTime::now());
    let daily = dir.join("Journal").join(format!("{today}.md"));
    assert!(daily.exists(), "Journal/YYYY-MM-DD.md without Obsidian's settings");
    assert_eq!(app.docs[app.active].path.as_ref(), Some(&daily));
    // Again: the same note opens; nothing is replaced.
    std::fs::write(&daily, "kept").unwrap();
    app.open_path(dir.join("Home.md"), false);
    app.run(&ctx, Cmd::NotesDailyNote);
    assert_eq!(std::fs::read_to_string(&daily).unwrap(), "kept");
    // A note from the template, named in the dialog.
    app.run(&ctx, Cmd::NotesNewFromTemplate);
    assert!(app.palette.open);
    frame(&mut app, &ctx, vec![press(Key::Enter, Modifiers::NONE)]);
    match app.notes.dialog.as_mut() {
        Some(super::notes_panel::Dialog::Name { name, .. }) => *name = "Standup".into(),
        _ => panic!("the name dialog is up"),
    }
    frame(&mut app, &ctx, vec![]);
    frame(&mut app, &ctx, vec![press(Key::Enter, Modifiers::NONE)]);
    let year = crate::notes::dates::format("YYYY", &crate::notes::dates::DateTime::now());
    assert_eq!(std::fs::read_to_string(dir.join("Standup.md")).unwrap(), format!("# Standup\nDate: {year}\n"));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn conflict_copies_get_a_notice_and_compare() {
    let dir = temp_dir("notes-conflict");
    write(&dir, "Plan.md", "mine\n");
    write(&dir, "Plan (conflict).md", "theirs\n");
    write(&dir, "Other.md", "[[Plan]]\n");
    let ctx = Context::default();
    let mut app = open_vault(&dir, "Plan.md", &ctx);
    let out = frame(&mut app, &ctx, vec![]);
    assert!(texts(&out).iter().any(|t| t.contains("“Plan (conflict).md” is another version of this note")));
    app.run(&ctx, Cmd::NotesCompareConflict);
    assert!(app.docs[app.active].compare.is_some(), "a Compare tab");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn preview_shows_wikilinks_tags_and_embeds() {
    let dir = temp_dir("notes-preview");
    write(&dir, "Home.md", "---\ntags: [home]\n---\n# Home\nSee [[Other|the other]] and [[Missing]] #topic\n\n![[Other#Part]]\n\n![[Home]]\n");
    write(&dir, "Other.md", "# Other\n## Part\nEmbedded text here\n## Next\nnot this\n");
    let ctx = Context::default();
    let mut app = open_vault(&dir, "Home.md", &ctx);
    app.settings.markdown_preview = true;
    let mut out = frame(&mut app, &ctx, vec![]);
    for _ in 0..4 {
        out = frame(&mut app, &ctx, vec![]);
    }
    let shown = texts(&out).join("\n");
    assert!(shown.contains("the other") && shown.contains("Missing") && shown.contains("#topic"), "{shown}");
    assert!(shown.contains("Embedded text here") && !shown.contains("not this"), "a section is embedded: {shown}");
    assert!(shown.contains("↪ Home"), "a note embedding itself shows a link");
    assert!(shown.contains("tags: [home]"), "frontmatter as properties");
    std::fs::remove_dir_all(dir).ok();
}
