//! Regression tests for saving, reloading and restoring without losing or
//! silently changing the user's text.

use super::*;

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("strata-safety-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn text(app: &StrataApp, i: usize) -> String {
    let b = &app.docs[i].editor.buffer;
    b.slice(0..b.len_chars())
}

#[test]
fn hot_exit_keeps_decoding_errors_and_the_line_ending() {
    let config = temp_dir("lossy");
    let file = config.join("damaged.txt");
    std::fs::write(&file, "one line").unwrap();
    let mut app = StrataApp::new(Some(file.clone()), Some(config.clone()));
    let flip = |e| if e == LineEnding::Lf { LineEnding::CrLf } else { LineEnding::Lf };
    let ending = flip(app.doc().editor.buffer.line_ending());
    // As if some bytes hadn't decoded; the next Enter uses the other ending.
    app.doc().editor.buffer.lossy = true;
    app.doc().editor.buffer.set_line_ending(ending);
    app.doc().editor.insert_text("edit ");
    assert!(app.save_session());
    drop(app);

    let back = StrataApp::new(None, Some(config.clone()));
    assert_eq!(text(&back, 0), "edit one line");
    assert!(back.docs[0].editor.buffer.lossy, "Save still warns about the replaced bytes");
    assert_eq!(back.docs[0].editor.buffer.line_ending(), ending);
    std::fs::remove_dir_all(config).ok();
}

#[test]
fn save_all_saves_a_buffer_with_two_views_once() {
    let dir = temp_dir("save-all-views");
    let path = dir.join("a.txt");
    std::fs::write(&path, "a  \nb\n").unwrap();
    let mut app = StrataApp::new(Some(path.clone()), None);
    app.settings.trim_trailing_white_space_on_save = true;
    app.clone_file();
    app.sync_views();
    // Restored from hot exit as unsaved (no edits in the shared history).
    for d in &mut app.docs {
        d.restored_dirty = true;
    }
    let steps = app.docs[0].editor.history_len();
    // Saving the second view would ask "changed by another program?" (a
    // dialog fails the test) and trim a second time.
    app.save_all();
    assert_eq!(app.status, "Saved 1 file");
    assert_eq!(app.docs[0].editor.history_len(), steps + 1, "the trim is one undo step");
    app.sync_views();
    assert!(app.docs.iter().all(|d| !d.is_dirty() && !d.changed_on_disk()));
    assert_eq!(text(&app, 1), "a\nb\n");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "a\nb\n");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn old_sessions_without_the_new_fields_still_load() {
    let tab: crate::session::TabState = serde_json::from_str(r#"{"path": "/x/a.txt", "backup": "1.txt"}"#).unwrap();
    assert!(!tab.lossy);
    assert_eq!(tab.line_ending, None);
}

#[cfg(unix)]
#[test]
fn saving_a_read_only_file_asks_first() {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp_dir("readonly");
    let ro = dir.join("ro.txt");
    std::fs::write(&ro, "locked").unwrap();
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o444)).unwrap();
    let rw = dir.join("rw.txt");
    std::fs::write(&rw, "open").unwrap();
    let mut app = StrataApp::new(None, None);
    app.open_path(ro.clone(), false);
    let i = app.active;
    assert_eq!(app.overwrite_reasons(i, &ro), vec![READ_ONLY_REASON]);
    // Save As to a writable file needs no question; Save As onto a read-only one does.
    assert!(app.overwrite_reasons(i, &rw).is_empty());
    app.open_path(rw.clone(), false);
    assert_eq!(app.overwrite_reasons(app.active, &ro), vec![READ_ONLY_REASON]);
    std::fs::remove_dir_all(dir).ok();
}
