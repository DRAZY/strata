//! Hot-exit autosave off the UI thread. Every few seconds the session (tabs,
//! panes, unsaved text) is written so a crash loses little; with a big
//! unsaved file that write takes a while, so it runs on a worker. The UI
//! thread only takes rope snapshots (O(1): chunks are shared) and names each
//! backup after its exact text, so unchanged tabs aren't written again.
//!
//! One write runs at a time in the whole process (several windows share the
//! session); a save that finds one still going is skipped until next time.
//! Saving on quit waits for it and then writes synchronously.

use super::StrataApp;
use crate::session::Session;
use std::path::PathBuf;
use std::sync::Mutex;
use std::thread::JoinHandle;

pub type Backups = Vec<(String, ropey::Rope)>;

static JOB: Mutex<Option<JoinHandle<Result<(), String>>>> = Mutex::new(None);
static FAILED: Mutex<Option<String>> = Mutex::new(None);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Collect a finished write (its error, if any, for `take_error`).
fn reap(job: &mut Option<JoinHandle<Result<(), String>>>, block: bool) {
    if job.as_ref().is_some_and(|j| block || j.is_finished())
        && let Some(j) = job.take()
        && let Err(e) = j.join().unwrap_or_else(|_| Err("the writer crashed".into()))
    {
        *lock(&FAILED) = Some(e);
    }
}

/// Write `session` and `backups` into `dir` on a worker. False (nothing
/// started) while an earlier write is still going.
pub fn start(session: Session, dir: PathBuf, backups: Backups) -> bool {
    let mut job = lock(&JOB);
    reap(&mut job, false);
    if job.is_some() {
        return false;
    }
    *job = Some(std::thread::spawn(move || session.save(&dir, &backups).map_err(|e| e.to_string())));
    true
}

/// Wait for the write in progress, if any.
pub fn wait() {
    reap(&mut lock(&JOB), true);
}

/// Why the last background write failed, once.
pub fn take_error() -> Option<String> {
    reap(&mut lock(&JOB), false);
    lock(&FAILED).take()
}

impl StrataApp {
    /// This window's session, ready to write; None when it isn't ours to save.
    pub(super) fn session_to_save(&self) -> Option<(PathBuf, Session, Backups)> {
        let dir = self.config.clone().filter(|_| self.session_lock.is_some())?;
        let mut backups = Vec::new();
        let mut session = Session::of_windows(vec![self.window_state(&mut backups)]);
        session.recent_files = self.recent_files.clone();
        session.recent_folders = self.recent_folders.clone();
        session.recent_projects = self.recent_projects.clone();
        Some((dir, session, backups))
    }

    /// Autosave: start writing the session if it changed and no write is going.
    pub(super) fn autosave(&mut self) {
        if let Some(e) = take_error() {
            self.status = format!("Could not save session: {e}");
            // Try again next time.
            self.session_key = 0;
        }
        if !self.hot_exit() {
            return;
        }
        let key = self.session_fingerprint();
        if key == self.session_key {
            return;
        }
        if let Some((dir, session, backups)) = self.session_to_save()
            && start(session, dir, backups)
        {
            self.session_key = key;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;

    #[test]
    fn autosave_writes_off_the_ui_thread_and_only_changed_tabs() {
        let config = std::env::temp_dir().join(format!("strata-autosave-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&config);
        std::fs::create_dir_all(&config).unwrap();
        let names = || {
            let mut v: Vec<String> = std::fs::read_dir(Session::backups(&config)).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
            v.sort();
            v
        };
        let sorted = |mut v: Vec<String>| {
            v.sort();
            v
        };
        let mut app = StrataApp::new(None, Some(config.clone()));
        app.docs[0].editor.insert_text("one");
        app.push_doc(Document::untitled());
        app.doc().editor.insert_text("two");
        app.autosave();
        wait();
        let one = Session::backup_name_for(app.docs[0].editor.text_id());
        let two = Session::backup_name_for(app.docs[1].editor.text_id());
        assert_eq!(names(), sorted(vec![one.clone(), two.clone()]));
        let written = std::fs::metadata(Session::backups(&config).join(&one)).unwrap().modified().unwrap();

        // Only the edited tab gets a new backup; its old one is pruned.
        app.docs[1].editor.insert_text("!");
        app.autosave();
        wait();
        let two_now = Session::backup_name_for(app.docs[1].editor.text_id());
        assert_ne!(two, two_now);
        assert_eq!(names(), sorted(vec![one.clone(), two_now]));
        assert_eq!(std::fs::metadata(Session::backups(&config).join(&one)).unwrap().modified().unwrap(), written, "not rewritten");

        drop(app);
        let back = StrataApp::new(None, Some(config.clone()));
        let texts: Vec<String> = back.docs.iter().map(|d| d.editor.buffer.slice(0..d.editor.buffer.len_chars())).collect();
        assert_eq!(texts, ["one", "two!"]);
        std::fs::remove_dir_all(config).ok();
    }
}
