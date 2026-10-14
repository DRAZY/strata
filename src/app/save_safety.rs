//! Regression tests for saving, reloading and restoring without losing or
//! silently changing the user's text.

use super::*;

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("strata-safety-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
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
