//! Git sync for notes, the window half (see `crate::notes::sync`): Notes:
//! Sync Notes, the question that turns sync on for a vault, the timer, the
//! status bar item and the list of notes in conflict.
//!
//! Unsaved notes: Sync Notes saves the vault's tabs first (it's a command
//! the user gave, like Save All on Build); the timer skips a vault with
//! unsaved notes and tries again next time, so it never commits a file
//! whose newest text is still only in a tab. While Git rewrites files (the
//! rebase), saves into the repository wait (`notes_sync_holds`).

use super::StrataApp;
use crate::commands::Cmd;
use crate::notes::sync::{self, Failure, Identity, Job, Remote, Resolution, VaultSync};
use egui::Color32;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;

/// Look at the timers at most this often (seconds).
const TIMER_CHECK: f64 = 1.0;
/// Longest timer interval offered, in minutes (a day).
const MAX_MINUTES: u32 = 24 * 60;
const AMBER: Color32 = Color32::from_rgb(0xe5, 0x9e, 0x40);

/// How the last sync of a vault went.
struct Last {
    /// Local time, "14:05".
    at: String,
    ok: bool,
    text: String,
    detail: Option<String>,
}

/// Notes in conflict after a sync, for the panel.
pub(super) struct ConflictView {
    /// The vault (its real path).
    vault: PathBuf,
    conflict: sync::Conflict,
}

/// "Enable Git Sync for This Vault?"
struct Enable {
    vault: PathBuf,
    name: String,
    /// The repository, its remote and its identity, being looked up.
    rx: Option<Receiver<Result<(Remote, Identity), Failure>>>,
    remote: Option<Result<Remote, Failure>>,
    /// What turning sync on is bound to (see `sync::Identity`).
    identity: Option<Identity>,
    /// Asked again because the folder now holds another repository.
    again: bool,
    timer: bool,
    minutes: u32,
}

#[derive(Default)]
pub(super) struct SyncState {
    job: Option<Job>,
    /// By vault (real path).
    last: HashMap<PathBuf, Last>,
    pub conflict: Option<ConflictView>,
    pub show_conflict: bool,
    /// When each vault's timer fires next (input time).
    due: HashMap<PathBuf, f64>,
    last_check: f64,
    enable: Option<Enable>,
    /// This window's number in `BUSY` (0: not given yet).
    window: u64,
    /// Syncs the timer started (tests).
    pub timer_starts: usize,
    /// Extra Git environment (tests: a Git config of their own).
    pub env: Vec<(String, String)>,
    /// Hears each sync's phases (tests).
    #[cfg(test)]
    pub on_phase: Option<std::sync::Arc<dyn Fn(sync::Phase) + Send + Sync>>,
}

impl SyncState {
    #[cfg(test)]
    pub fn running(&self) -> bool {
        self.job.is_some()
    }

    /// This window's number in `BUSY`.
    fn window(&mut self) -> u64 {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        if self.window == 0 {
            self.window = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        self.window
    }
}

impl Drop for SyncState {
    fn drop(&mut self) {
        if self.window != 0 {
            BUSY.lock().unwrap_or_else(|e| e.into_inner()).remove(&self.window);
        }
    }
}

/// What each window (all windows share the process) is busy with that a
/// sync must wait for, so one window's timer doesn't start under another's rename.
static BUSY: std::sync::LazyLock<std::sync::Mutex<HashMap<u64, &'static str>>> = std::sync::LazyLock::new(Default::default);

/// `p` with links resolved; a file that doesn't exist yet by its folder.
fn real(p: &Path) -> PathBuf {
    if let Ok(r) = std::fs::canonicalize(p) {
        return plain(r);
    }
    match (p.parent().and_then(|d| std::fs::canonicalize(d).ok()), p.file_name()) {
        (Some(d), Some(n)) => plain(d).join(n),
        _ => p.to_owned(),
    }
}

/// Windows' `\\?\C:\…` and `\\?\UNC\server\…` (what `canonicalize` gives)
/// as ordinary paths: Git (as a working folder) and settings.json read
/// those better. Other paths are unchanged.
fn plain(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    match s.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => PathBuf::from(rest),
        _ => p,
    }
}

fn folder_name(p: &Path) -> String {
    p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned())
}

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

fn clock() -> String {
    crate::notes::dates::format("HH:mm", &crate::notes::dates::DateTime::now())
}

/// The status line for an outcome.
fn describe_outcome(o: &sync::Outcome) -> (bool, String, Option<String>) {
    match &o.result {
        sync::Result::Synced { committed: 0, pulled: 0, pushed: 0 } => (true, "Notes are up to date".into(), None),
        sync::Result::Synced { committed, pulled, pushed } => {
            let mut parts = Vec::new();
            if *committed > 0 {
                parts.push(format!("committed {}", plural(*committed, "file")));
            }
            if *pulled > 0 {
                parts.push(format!("brought in {}", plural(*pulled, "commit")));
            }
            if *pushed > 0 {
                parts.push("pushed".to_owned());
            }
            (true, format!("Synced notes: {}", parts.join(", ")), None)
        }
        sync::Result::Conflict(c) => {
            let n = plural(c.paths.len(), "note");
            (false, format!("Notes didn't sync: {n} changed here and on {} (nothing was pulled or pushed)", c.upstream), None)
        }
        sync::Result::Failed(f) => {
            let mut text = f.message.clone();
            if o.committed > 0 {
                text.push_str(&format!(" ({} committed here; they go up with the next sync)", plural(o.committed, "file")));
            }
            (false, text, f.detail.clone())
        }
    }
}

impl StrataApp {
    /// The `notes_git_sync` key for the vault at `vault` (a real path): the
    /// one Strata wrote, or one typed into settings.json naming the same folder another way.
    fn sync_key(&self, vault: &Path) -> Option<String> {
        let exact = vault.to_string_lossy();
        if self.settings.notes_git_sync.contains_key(&*exact) {
            return Some(exact.into_owned());
        }
        self.settings.notes_git_sync.keys().find(|k| real(Path::new(k.as_str())) == vault).cloned()
    }

    /// The real path of the vault holding `path`, when Git sync is on for it.
    fn synced_vault(&mut self, path: &Path) -> Option<PathBuf> {
        let root = self.vault_root(path)?;
        let vault = real(&root);
        self.sync_key(&vault).map(|_| vault)
    }

    fn vault_setting(&self, vault: &Path) -> Option<&VaultSync> {
        self.sync_key(vault).and_then(|k| self.settings.notes_git_sync.get(&k))
    }

    /// Why a sync shouldn't start now, if it shouldn't (in this window or another).
    fn sync_blocked(&self) -> Option<&'static str> {
        let me = self.notes.sync.window;
        // (Tests run many apps side by side in one process: each is on its own.)
        let others = !cfg!(test);
        self.busy_here().or_else(|| if others { BUSY.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(w, _)| **w != me).map(|(_, why)| *why) } else { None })
    }

    /// What this window is busy with that a sync must wait for.
    fn busy_here(&self) -> Option<&'static str> {
        if self.notes.busy() {
            Some("a rename's link updates (or their undo) are running")
        } else if self.review.busy() {
            Some("Review Changes is running")
        } else if self.sidebar.disk.running() {
            Some("Replace in Files is writing files")
        } else {
            None
        }
    }

    /// Tabs with unsaved changes inside `vault` (real path): their index.
    fn unsaved_in(&self, vault: &Path) -> Vec<usize> {
        (0..self.docs.len())
            .filter(|&i| {
                let d = &self.docs[i];
                let earlier_view = d.view_group.is_some() && (0..i).any(|k| self.docs[k].editor.shares_history(&d.editor));
                !earlier_view && d.is_dirty() && d.path.as_deref().is_some_and(|p| real(p).starts_with(vault))
            })
            .collect()
    }

    /// Git is rewriting files of the repository `path` is in: hold its save.
    pub(super) fn notes_sync_holds(&self, path: &Path) -> bool {
        sync::holds(path)
    }

    pub(super) fn notes_sync_command(&mut self, ctx: &egui::Context, cmd: Cmd) {
        match cmd {
            Cmd::NotesSync => self.sync_now(ctx, None),
            Cmd::NotesSyncEnable => self.ask_enable_sync(ctx),
            Cmd::NotesSyncDisable => self.disable_sync(),
            _ => {}
        }
    }

    /// Notes: Sync Notes for the vault Notes commands act on (with
    /// `resolve`: Keep My Versions and Sync).
    fn sync_now(&mut self, ctx: &egui::Context, resolve: Option<Resolution>) {
        let Some(root) = self.command_root() else { return };
        let vault = real(&root);
        if self.sync_key(&vault).is_none() {
            // Not on yet: the question that turns it on says what it does.
            return self.ask_enable_sync(ctx);
        }
        if self.notes.sync.job.is_some() {
            self.status = "Notes are syncing already".into();
            return;
        }
        if let Some(why) = self.sync_blocked() {
            self.status = format!("Didn't sync: {why}; sync again when it's done");
            return;
        }
        // Save the vault's notes first, so what's committed is what you see.
        self.sync_views();
        let mut failed = Vec::new();
        for i in self.unsaved_in(&vault) {
            if !self.save(i, false) {
                failed.push(self.docs[i].name());
            }
        }
        self.sync_views();
        if !failed.is_empty() {
            self.status = format!("Didn't sync: couldn't save {}", failed.join(", "));
            return;
        }
        self.start_sync(ctx, vault, true, resolve);
    }

    fn start_sync(&mut self, ctx: &egui::Context, vault: PathBuf, manual: bool, resolve: Option<Resolution>) {
        // Bound to the repository it was turned on for: checked before Git runs anything.
        let expect = Some(self.vault_setting(&vault).map(|v| v.repository.clone()).unwrap_or_default());
        #[allow(unused_mut)]
        let mut opts = sync::Options { message: self.settings.notes_sync_message.clone(), resolve, env: self.notes.sync.env.clone(), expect, ..Default::default() };
        #[cfg(test)]
        {
            opts.on_phase = self.notes.sync.on_phase.clone();
        }
        match Job::start(&vault, opts, manual, Some(ctx.clone())) {
            Some(job) => {
                if manual {
                    self.status = format!("Syncing {}…", folder_name(&vault));
                } else {
                    self.notes.sync.timer_starts += 1;
                }
                self.notes.sync.job = Some(job);
            }
            // The other window's sync covers this one's timer too.
            None if manual => self.status = format!("{} is being synced in another window", folder_name(&vault)),
            None => {}
        }
    }

    /// Every frame: a finished sync, the timers, the windows.
    pub(super) fn notes_sync_frame(&mut self, ctx: &egui::Context) {
        let (window, busy) = (self.notes.sync.window(), self.busy_here());
        {
            let mut all = BUSY.lock().unwrap_or_else(|e| e.into_inner());
            match busy {
                Some(why) => all.insert(window, why),
                None => all.remove(&window),
            };
        }
        self.sync_poll(ctx);
        self.sync_timers(ctx);
        self.enable_window(ctx);
        self.conflict_window(ctx);
    }

    fn sync_poll(&mut self, ctx: &egui::Context) {
        let Some(outcome) = self.notes.sync.job.as_ref().and_then(Job::poll) else { return };
        let Some(job) = self.notes.sync.job.take() else { return };
        let vault = job.vault.clone();
        let (ok, text, detail) = describe_outcome(&outcome);
        // Tabs, the gutter, the side bar and the index see what Git changed.
        let mut first = None;
        for i in 0..self.docs.len() {
            let Some(p) = self.docs[i].path.clone() else { continue };
            if !real(&p).starts_with(&vault) {
                continue;
            }
            first.get_or_insert(p.clone());
            self.docs[i].check_disk();
            self.git.invalidate(&p);
            self.docs[i].git_version = None;
        }
        if let Some(p) = first {
            self.notes_saved(&p);
        }
        self.sidebar.refresh();
        if let sync::Result::Failed(f) = &outcome.result
            && f.kind == sync::Kind::Untrusted
        {
            // Another repository at this path: off, and the question again.
            if let Some(key) = self.sync_key(&vault) {
                let minutes = self.settings.notes_git_sync.remove(&key).map_or(0, |v| v.interval_minutes);
                self.persist_settings();
                self.ask_enable_for(ctx, vault.clone(), minutes, true);
            }
            self.notes.sync.due.remove(&vault);
        }
        match outcome.result {
            sync::Result::Conflict(conflict) => {
                self.notes.sync.show_conflict = job.manual || self.notes.sync.show_conflict;
                self.notes.sync.conflict = Some(ConflictView { vault: vault.clone(), conflict });
            }
            _ if self.notes.sync.conflict.as_ref().is_some_and(|c| c.vault == vault) && ok => {
                self.notes.sync.conflict = None;
                self.notes.sync.show_conflict = false;
            }
            _ => {}
        }
        if job.manual || !ok {
            self.status = text.clone();
        }
        self.notes.sync.last.insert(vault, Last { at: clock(), ok, text, detail });
        ctx.request_repaint();
    }

    /// Vaults with a timer that are open in this window: (real path, minutes).
    fn timed_vaults(&mut self) -> Vec<(PathBuf, u32)> {
        let timed: Vec<(PathBuf, u32)> = self.settings.notes_git_sync.iter().filter(|(_, s)| s.interval_minutes > 0).map(|(k, s)| (real(Path::new(k)), s.interval_minutes.min(MAX_MINUTES))).collect();
        if timed.is_empty() {
            return timed;
        }
        let mut open: Vec<PathBuf> = self.sidebar.roots.iter().map(|r| real(r)).collect();
        open.extend(self.docs.iter().filter_map(|d| d.path.as_deref()).map(real));
        timed.into_iter().filter(|(v, _)| open.iter().any(|p| p.starts_with(v) || v.starts_with(p))).collect()
    }

    fn sync_timers(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        if now - self.notes.sync.last_check < TIMER_CHECK && self.notes.sync.last_check > 0.0 {
            return;
        }
        self.notes.sync.last_check = now;
        let vaults = self.timed_vaults();
        self.notes.sync.due.retain(|v, _| vaults.iter().any(|(t, _)| t == v));
        let mut next: Option<f64> = None;
        for (vault, minutes) in vaults {
            let every = f64::from(minutes) * 60.0;
            let due = *self.notes.sync.due.entry(vault.clone()).or_insert(now + every);
            if now < due {
                next = Some(next.map_or(due, |n| n.min(due)));
                continue;
            }
            // Due: the next run is a full interval away, whether this one starts or not.
            self.notes.sync.due.insert(vault.clone(), now + every);
            next = Some(next.map_or(now + every, |n| n.min(now + every)));
            let why = if self.notes.sync.job.is_some() {
                None // one sync at a time; this one is simply skipped
            } else if self.notes.sync.conflict.as_ref().is_some_and(|c| c.vault == vault) {
                Some("notes are in conflict; settle them first".to_owned())
            } else if let Some(why) = self.sync_blocked() {
                Some(format!("{why}; trying again later"))
            } else if !self.unsaved_in(&vault).is_empty() {
                Some("waiting for unsaved notes to be saved".to_owned())
            } else {
                self.start_sync(ctx, vault.clone(), false, None);
                None
            };
            if let Some(why) = why {
                let at = clock();
                let last = self.notes.sync.last.entry(vault).or_insert(Last { at: at.clone(), ok: true, text: String::new(), detail: None });
                last.detail = Some(format!("Timer skipped at {at}: {why}"));
            }
        }
        if let Some(n) = next {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64((n - now).clamp(0.1, 3600.0)));
        }
    }

    /// The status bar item for the current note's vault: (text, color, tooltip).
    pub(super) fn notes_sync_status(&mut self) -> Option<(String, Option<Color32>, String)> {
        let path = self.docs[self.active].path.clone()?;
        let vault = self.synced_vault(&path)?;
        let s = &self.notes.sync;
        if s.job.as_ref().is_some_and(|j| j.vault == vault) {
            return Some(("Syncing…".into(), None, "Git sync is running for this vault".into()));
        }
        if s.conflict.as_ref().is_some_and(|c| c.vault == vault) {
            return Some(("Sync: conflict".into(), Some(AMBER), "Notes changed here and on the remote. Click to see them.".into()));
        }
        let every = match self.vault_setting(&vault).map_or(0, |v| v.interval_minutes) {
            0 => "when you sync".to_owned(),
            m => format!("every {}", plural(m as usize, "minute")),
        };
        let s = &self.notes.sync;
        Some(match s.last.get(&vault) {
            Some(l) if !l.text.is_empty() => {
                let mut tip = format!("{} (at {}).", l.text, l.at);
                if let Some(d) = &l.detail {
                    tip.push_str(&format!("\n{d}"));
                }
                tip.push_str(&format!("\nGit sync runs {every}. Click to sync now."));
                if l.ok { (format!("Synced {}", l.at), None, tip) } else { ("Sync failed".into(), Some(AMBER), tip) }
            }
            last => {
                let skipped = last.and_then(|l| l.detail.clone()).map(|d| format!("\n{d}")).unwrap_or_default();
                ("Not synced yet".into(), None, format!("Git sync is on for this vault and runs {every}. Click to sync now.{skipped}"))
            }
        })
    }

    pub(super) fn notes_sync_clicked(&mut self, ctx: &egui::Context) {
        let conflict_here = self.docs[self.active].path.clone().and_then(|p| self.synced_vault(&p)).is_some_and(|v| self.notes.sync.conflict.as_ref().is_some_and(|c| c.vault == v));
        if conflict_here {
            self.notes.sync.show_conflict = true;
        } else {
            self.sync_now(ctx, None);
        }
    }

    // ---- turning it on and off ------------------------------------------------------------

    fn ask_enable_sync(&mut self, ctx: &egui::Context) {
        let Some(root) = self.command_root() else { return };
        let vault = real(&root);
        let current = self.vault_setting(&vault).map_or(0, |v| v.interval_minutes);
        self.ask_enable_for(ctx, vault, current, false);
    }

    /// "Enable Git sync?" for the vault at `vault` (a real path).
    fn ask_enable_for(&mut self, ctx: &egui::Context, vault: PathBuf, current: u32, again: bool) {
        let (tx, rx) = std::sync::mpsc::channel();
        let (dir, ctx2) = (vault.clone(), ctx.clone());
        // Looking the remote up runs Git: off the UI thread.
        std::thread::spawn(move || {
            let _ = tx.send(sync::describe(&dir).and_then(|r| sync::identity(&dir).map(|i| (r, i))));
            ctx2.request_repaint();
        });
        self.notes.sync.enable = Some(Enable { name: folder_name(&vault), vault, rx: Some(rx), remote: None, identity: None, again, timer: current > 0, minutes: if current > 0 { current } else { 15 } });
    }

    fn disable_sync(&mut self) {
        let Some(root) = self.command_root() else { return };
        let vault = real(&root);
        let Some(key) = self.sync_key(&vault) else {
            self.status = format!("Git sync isn't on for {}", folder_name(&root));
            return;
        };
        self.settings.notes_git_sync.remove(&key);
        self.persist_settings();
        self.notes.sync.due.remove(&vault);
        self.notes.sync.last.remove(&vault);
        if self.notes.sync.conflict.as_ref().is_some_and(|c| c.vault == vault) {
            self.notes.sync.conflict = None;
            self.notes.sync.show_conflict = false;
        }
        self.status = format!("Git sync is off for {}", folder_name(&root));
    }

    fn enable_window(&mut self, ctx: &egui::Context) {
        let Some(e) = self.notes.sync.enable.as_mut() else { return };
        if let Some(r) = e.rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            e.remote = Some(r.map(|(remote, identity)| {
                e.identity = Some(identity);
                remote
            }));
            e.rx = None;
        }
        let (mut enable, mut close) = (false, false);
        egui::Window::new("Enable Git Sync")
            .id(self.link.id("notes-sync-enable"))
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .collapsible(false)
            .resizable(false)
            .title_bar(false)
            .show(ctx, |ui| {
                let size = egui::TextStyle::Body.resolve(ui.style()).size;
                ui.set_max_width((size * 34.0).max(440.0));
                ui.strong(format!("Enable Git sync for “{}”?", e.name));
                if e.again {
                    ui.colored_label(AMBER, "This isn't the repository Git sync was turned on for here (it was replaced, its remote changed, or sync was added to settings.json by hand), so sync is off until you say again.");
                }
                ui.label(egui::RichText::new(e.vault.display().to_string()).weak());
                ui.add_space(4.0);
                match &e.remote {
                    None => {
                        ui.label("Looking at the repository…");
                    }
                    Some(Err(f)) => {
                        ui.label(&f.message);
                    }
                    Some(Ok(r)) => {
                        let scope = if r.prefix.is_empty() { "the changed files in this folder".to_owned() } else { format!("the changed files in this folder (only {}, not the rest of the repository)", r.prefix.trim_end_matches('/')) };
                        ui.label(format!("Strata will commit {scope}, bring in new commits from {} with a rebase, and push to {} on branch {}.", r.upstream, r.url, r.branch));
                        ui.label("Git runs here with this repository's own settings, as it does in a terminal: its hooks, filters (such as Git LFS) and credential helpers will run. Turn sync on only for a repository you set up yourself.");
                        ui.label("Strata connects to the remote only when you choose Notes: Sync Notes, or on the timer below. It never asks for a password: Git uses your credential helper or SSH agent.");
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut e.timer, "Also sync every");
                            ui.add_enabled(e.timer, egui::DragValue::new(&mut e.minutes).range(1..=MAX_MINUTES));
                            ui.label("minutes while the vault is open");
                        });
                    }
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if matches!(e.remote, Some(Ok(_))) && ui.button("Enable Sync").clicked() {
                        enable = true;
                    }
                    if ui.button(if matches!(e.remote, Some(Ok(_))) { "Cancel" } else { "Close" }).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        close = true;
                    }
                });
            });
        if enable {
            self.enable_answered(true);
        } else if close {
            self.notes.sync.enable = None;
        }
    }

    /// The answer to "Enable Git sync?" (tests press the button through this).
    pub(super) fn enable_answered(&mut self, yes: bool) {
        let Some(e) = self.notes.sync.enable.take() else { return };
        let (true, Some(Ok(_)), Some(identity)) = (yes, &e.remote, e.identity.clone()) else { return };
        let minutes = if e.timer { e.minutes.clamp(1, MAX_MINUTES) } else { 0 };
        if let Some(old) = self.sync_key(&e.vault) {
            self.settings.notes_git_sync.remove(&old);
        }
        self.settings.notes_git_sync.insert(e.vault.to_string_lossy().into_owned(), VaultSync { interval_minutes: minutes, repository: identity });
        self.persist_settings();
        self.notes.sync.due.remove(&e.vault);
        let when = if minutes > 0 { format!("every {} and when you choose Notes: Sync Notes", plural(minutes as usize, "minute")) } else { "when you choose Notes: Sync Notes".to_owned() };
        self.status = format!("Git sync is on for {}: it runs {when}", e.name);
    }

    /// The remote's description has arrived (tests).
    #[cfg(test)]
    pub(super) fn enable_ready(&self) -> bool {
        self.notes.sync.enable.as_ref().is_some_and(|e| e.remote.is_some())
    }

    // ---- conflicts --------------------------------------------------------------------------

    fn conflict_window(&mut self, ctx: &egui::Context) {
        if !self.notes.sync.show_conflict {
            return;
        }
        let Some(cv) = self.notes.sync.conflict.as_ref() else { return };
        let c = &cv.conflict;
        let vault_rel = |rel: &str| -> String {
            let p = c.top.join(rel);
            p.strip_prefix(&cv.vault).map(|r| r.to_string_lossy().into_owned()).unwrap_or_else(|_| rel.to_owned())
        };
        let (mut compare, mut keep, mut close) = (None, false, false);
        egui::Window::new("Notes in conflict")
            .id(self.link.id("notes-sync-conflict"))
            .anchor(egui::Align2::RIGHT_BOTTOM, [-16.0, -40.0])
            .collapsible(false)
            .resizable(false)
            .title_bar(false)
            .show(ctx, |ui| {
                let size = egui::TextStyle::Body.resolve(ui.style()).size;
                ui.set_max_width((size * 34.0).max(440.0));
                ui.strong("Notes didn't sync");
                ui.label(format!("{} changed here and on {} since the last sync, so nothing was pulled or pushed. Your files are as they were.", if c.paths.len() == 1 { "This note".to_owned() } else { format!("These {} notes", c.paths.len()) }, c.upstream));
                if c.moved {
                    ui.colored_label(AMBER, format!("{} has changed them again since you compared: compare again before keeping your versions.", c.upstream));
                }
                ui.add_space(4.0);
                egui::ScrollArea::vertical().max_height(size * 12.0).show(ui, |ui| {
                    for rel in &c.paths {
                        ui.horizontal(|ui| {
                            ui.label(vault_rel(rel));
                            if ui.small_button(format!("Compare with {}", c.upstream)).clicked() {
                                compare = Some(rel.clone());
                            }
                        });
                    }
                });
                if !c.copies.is_empty() {
                    let names: Vec<String> = c.copies.iter().map(|p| folder_name(p)).collect();
                    ui.label(format!("Text another program wrote while Git worked was kept as {}.", names.join(", ")));
                }
                ui.add_space(4.0);
                ui.label("To finish: in each comparison, bring over what you want to keep from the remote's side, and save. Then choose Keep My Versions and Sync: Strata records a merge that keeps these notes exactly as they are on your disk (the remote's versions stay in the history) and pushes it.");
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    keep = ui.button("Keep My Versions and Sync").clicked();
                    close = ui.button("Close").clicked();
                });
            });
        if let Some(rel) = compare {
            self.compare_with_remote(&rel);
        } else if keep {
            self.keep_my_versions(ctx);
        } else if close {
            self.notes.sync.show_conflict = false;
        }
    }

    /// Open the note `rel` (from the repository's top) in Compare, against
    /// the remote's version of it.
    pub(super) fn compare_with_remote(&mut self, rel: &str) {
        let Some(cv) = self.notes.sync.conflict.as_ref() else { return };
        let (path, top, commit, upstream) = (cv.conflict.top.join(rel), cv.conflict.top.clone(), cv.conflict.commit.clone(), cv.conflict.upstream.clone());
        let name = folder_name(&path);
        if !path.is_file() {
            self.status = format!("{name} was deleted here; {upstream} still has it");
            return;
        }
        let remote = crate::git::git_io(&top, &["cat-file", "blob", &format!("{commit}:{rel}")], None, 64 << 20, std::time::Duration::from_secs(10));
        let (text, label) = match remote {
            Some(bytes) => {
                let b = crate::buffer::Buffer::decode(&bytes).0;
                (b.slice(0..b.len_chars()), format!("{name} ({upstream})"))
            }
            None => (String::new(), format!("{name} (deleted on {upstream})")),
        };
        let r = match self.tab_for_path(&path) {
            Some(r) => r,
            None => {
                self.open_path(path.clone(), false);
                let Some(r) = self.docs.iter().position(|d| d.path.as_ref() == Some(&path)) else { return };
                r
            }
        };
        let syntax = self.docs[r].highlighter.syntax_name().to_owned();
        self.open_compare(r, &text, label, None, &syntax, None);
    }

    fn keep_my_versions(&mut self, ctx: &egui::Context) {
        let Some(cv) = self.notes.sync.conflict.as_ref() else { return };
        let resolve = Resolution { paths: cv.conflict.paths.clone(), commit: cv.conflict.commit.clone() };
        let vault = cv.vault.clone();
        // The vault Notes commands act on must be the one in conflict.
        if self.command_root().map(|r| real(&r)) != Some(vault.clone()) {
            self.status = format!("Open a note in {} first", folder_name(&vault));
            return;
        }
        self.notes.sync.show_conflict = false;
        self.sync_now(ctx, Some(resolve));
    }
}

#[cfg(test)]
mod tests {
    use super::super::ui_tests::frame;
    use super::*;
    use crate::notes::sync::repo_tests::{Fixture, fixture, g, git_env, read, write};
    use egui::{Context, Event};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    /// Every text drawn in `out`.
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

    fn shows(out: &egui::FullOutput, needle: &str) -> bool {
        texts(out).iter().any(|t| t.contains(needle))
    }

    /// The vault of `f` open with Plan.md, Git sync not on yet.
    fn open(f: &Fixture, ctx: &Context) -> StrataApp {
        let mut app = StrataApp::new(Some(f.vault()), None);
        app.notes.sync.env = git_env(&f.config);
        app.open_path(f.vault().join("Plan.md"), false);
        for _ in 0..3 {
            frame(&mut app, ctx, vec![]);
            app.notes.index.wait();
        }
        app
    }

    fn enable(app: &mut StrataApp, minutes: u32) {
        let key = real(&app.sidebar.roots[0]).to_string_lossy().into_owned();
        let repository = sync::identity(&app.sidebar.roots[0]).expect("a repository");
        app.settings.notes_git_sync.insert(key, VaultSync { interval_minutes: minutes, repository });
    }

    /// Frames until no sync runs.
    fn finish(app: &mut StrataApp, ctx: &Context) -> egui::FullOutput {
        let deadline = Instant::now() + Duration::from_secs(60);
        while app.notes.sync.running() {
            assert!(Instant::now() < deadline, "the sync didn't finish (status: {})", app.status);
            frame(app, ctx, vec![]);
            std::thread::sleep(Duration::from_millis(10));
        }
        frame(app, ctx, vec![])
    }

    fn type_text(app: &mut StrataApp, ctx: &Context, text: &str) {
        frame(app, ctx, vec![Event::Text(text.into())]);
    }

    fn vault_key(app: &StrataApp) -> PathBuf {
        real(&app.sidebar.roots[0])
    }

    #[test]
    fn windows_verbatim_paths_become_plain() {
        assert_eq!(plain(PathBuf::from(r"\\?\C:\Users\me\Notes")), PathBuf::from(r"C:\Users\me\Notes"));
        assert_eq!(plain(PathBuf::from(r"\\?\UNC\server\share\Notes")), PathBuf::from(r"\\server\share\Notes"));
        assert_eq!(plain(PathBuf::from(r"\\?\Volume{1234}\x")), PathBuf::from(r"\\?\Volume{1234}\x"));
        assert_eq!(plain(PathBuf::from("/Users/me/Notes")), PathBuf::from("/Users/me/Notes"));
    }

    #[test]
    fn sync_notes_asks_first_then_saves_commits_and_shows_the_status() {
        let Some(f) = fixture("ui-sync") else { return };
        let ctx = Context::default();
        let mut app = open(&f, &ctx);
        // Not on yet: Sync Notes asks, saying what will run.
        app.run(&ctx, Cmd::NotesSync);
        let deadline = Instant::now() + Duration::from_secs(20);
        while !app.enable_ready() {
            assert!(Instant::now() < deadline);
            frame(&mut app, &ctx, vec![]);
        }
        let out = frame(&mut app, &ctx, vec![]);
        assert!(shows(&out, "Enable Git sync for “vault”?"), "{:?}", texts(&out));
        assert!(shows(&out, "hooks, filters (such as Git LFS) and credential helpers will run"));
        assert!(shows(&out, "bring in new commits from origin/main"));
        assert!(app.settings.notes_git_sync.is_empty(), "nothing is on until the answer");
        app.enable_answered(true);
        assert!(app.settings.notes_git_sync.contains_key(&*vault_key(&app).to_string_lossy()));
        assert!(app.status.contains("Git sync is on for vault"), "{}", app.status);
        let out = frame(&mut app, &ctx, vec![]);
        assert!(shows(&out, "Not synced yet"), "the status bar says it's on");
        // An unsaved edit: Sync Notes saves it first, then commits and pushes it.
        type_text(&mut app, &ctx, "typed in Strata ");
        assert!(app.docs[app.active].is_dirty());
        app.run(&ctx, Cmd::NotesSync);
        assert!(!app.docs[app.active].is_dirty(), "saved before syncing");
        let out = finish(&mut app, &ctx);
        assert_eq!(app.status, "Synced notes: committed 1 file, pushed");
        assert!(texts(&out).iter().any(|t| t.starts_with("Synced ")), "{:?}", texts(&out));
        assert!(f.remote_text("vault/Plan.md").starts_with("typed in Strata # Plan"));
        // From the other device: pulled into the open (clean) tab.
        f.there_commits("vault/Plan.md", "rewritten there\n");
        app.run(&ctx, Cmd::NotesSync);
        finish(&mut app, &ctx);
        assert_eq!(app.status, "Synced notes: brought in 1 commit");
        let b = &app.docs[app.active].editor.buffer;
        let d = &app.docs[app.active];
        assert_eq!(b.slice(0..b.len_chars()), "rewritten there\n", "the tab shows the pulled text (dirty {}, conflict {}, disk {:?})", d.is_dirty(), d.conflict, read(&f.vault(), "Plan.md"));
        // Off again.
        app.run(&ctx, Cmd::NotesSyncDisable);
        assert!(app.settings.notes_git_sync.is_empty());
        assert_eq!(app.status, "Git sync is off for vault");
        let out = frame(&mut app, &ctx, vec![]);
        assert!(!texts(&out).iter().any(|t| t.starts_with("Synced ")));
    }

    #[test]
    fn a_different_repository_at_the_same_path_is_asked_about_again() {
        let Some(f) = fixture("ui-identity") else { return };
        let ctx = Context::default();
        let mut app = open(&f, &ctx);
        app.run(&ctx, Cmd::NotesSyncEnable);
        while !app.enable_ready() {
            frame(&mut app, &ctx, vec![]);
        }
        app.enable_answered(true);
        assert_eq!(app.settings.notes_git_sync.len(), 1);
        // The folder is replaced by another repository (with a remote of its own).
        std::fs::remove_dir_all(&f.here).unwrap();
        g(&f, &f.base, &["init", "-q", "--bare", "-b", "main", "other-remote.git"]);
        g(&f, &f.base, &["clone", "-q", f.base.join("other-remote.git").to_str().unwrap(), "here"]);
        write(&f.vault(), "Plan.md", "someone else's\n");
        g(&f, &f.here, &["add", "."]);
        g(&f, &f.here, &["commit", "-q", "-m", "x"]);
        g(&f, &f.here, &["push", "-q", "-u", "origin", "main"]);
        let head = g(&f, &f.here, &["rev-parse", "HEAD"]);
        write(&f.vault(), "Plan.md", "changed\n");
        app.run(&ctx, Cmd::NotesSync);
        finish(&mut app, &ctx);
        assert_eq!(g(&f, &f.here, &["rev-parse", "HEAD"]), head, "nothing committed");
        assert!(app.settings.notes_git_sync.is_empty(), "turned off");
        while !app.enable_ready() {
            frame(&mut app, &ctx, vec![]);
        }
        let out = frame(&mut app, &ctx, vec![]);
        assert!(shows(&out, "the repository Git sync was turned on for"), "{:?}", texts(&out));
        assert!(shows(&out, "hooks, filters (such as Git LFS) and credential helpers will run"), "the disclosure again");
        // The timer path is refused the same way (the check is in the sync itself).
        app.enable_answered(true);
        assert_eq!(app.settings.notes_git_sync.len(), 1, "on again for the new repository, after asking");
    }

    #[test]
    fn a_folder_without_a_remote_is_explained_and_not_turned_on() {
        let Some(f) = fixture("ui-noremote") else { return };
        g(&f, &f.here, &["checkout", "-q", "-b", "local-only"]);
        let ctx = Context::default();
        let mut app = open(&f, &ctx);
        app.run(&ctx, Cmd::NotesSyncEnable);
        while !app.enable_ready() {
            frame(&mut app, &ctx, vec![]);
        }
        let out = frame(&mut app, &ctx, vec![]);
        assert!(shows(&out, "has no upstream on a remote"), "{:?}", texts(&out));
        assert!(!shows(&out, "Enable Sync"), "no way to turn it on");
        app.enable_answered(true);
        assert!(app.settings.notes_git_sync.is_empty());
    }

    #[test]
    fn the_timer_never_overlaps_skips_unsaved_notes_and_waits_for_review() {
        let Some(f) = fixture("ui-timer") else { return };
        let ctx = Context::default();
        let mut app = open(&f, &ctx);
        enable(&mut app, 5);
        let vault = vault_key(&app);
        // A manual sync held at Fetch.
        let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
        let go_rx = Arc::new(Mutex::new(go_rx));
        let hold = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let (h, rx) = (hold.clone(), go_rx.clone());
        app.notes.sync.on_phase = Some(Arc::new(move |p| {
            if p == sync::Phase::Fetch && h.load(std::sync::atomic::Ordering::SeqCst) {
                let _ = rx.lock().unwrap().recv();
            }
        }));
        write(&f.vault(), "Ideas.md", "changed\n");
        app.run(&ctx, Cmd::NotesSync);
        assert!(app.notes.sync.running());
        // The timer comes due while it runs: nothing else starts.
        for _ in 0..3 {
            app.notes.sync.due.insert(vault.clone(), 0.0);
            app.notes.sync.last_check = 0.0;
            frame(&mut app, &ctx, vec![]);
        }
        assert_eq!(app.notes.sync.timer_starts, 0, "never two at once");
        hold.store(false, std::sync::atomic::Ordering::SeqCst);
        go_tx.send(()).unwrap();
        finish(&mut app, &ctx);
        assert!(app.status.starts_with("Synced notes"), "{}", app.status);
        // An unsaved note in the vault: the timer skips this round.
        type_text(&mut app, &ctx, "unsaved ");
        let remote = f.remote_head();
        app.notes.sync.due.insert(vault.clone(), 0.0);
        app.notes.sync.last_check = 0.0;
        frame(&mut app, &ctx, vec![]);
        assert!(!app.notes.sync.running() && app.notes.sync.timer_starts == 0);
        assert!(app.notes.sync.last[&vault].detail.as_deref().unwrap().contains("waiting for unsaved notes"));
        assert_eq!(f.remote_head(), remote);
        assert!(app.notes.sync.due[&vault] > 200.0, "the next try is an interval away");
        // Saved: the timer runs it.
        assert!(app.save(app.active, false));
        app.notes.sync.due.insert(vault.clone(), 0.0);
        app.notes.sync.last_check = 0.0;
        frame(&mut app, &ctx, vec![]);
        assert_eq!(app.notes.sync.timer_starts, 1);
        finish(&mut app, &ctx);
        assert!(f.remote_text("vault/Plan.md").starts_with("unsaved # Plan"));
        // Review Changes running: the timer waits.
        app.review_start(Some(&ctx));
        let deadline = Instant::now() + Duration::from_secs(20);
        while !app.review.busy() {
            assert!(Instant::now() < deadline);
            frame(&mut app, &ctx, vec![]);
            std::thread::sleep(Duration::from_millis(10));
        }
        write(&f.vault(), "Ideas.md", "changed during the review\n");
        app.notes.sync.due.insert(vault.clone(), 0.0);
        app.notes.sync.last_check = 0.0;
        frame(&mut app, &ctx, vec![]);
        assert_eq!(app.notes.sync.timer_starts, 1);
        assert!(app.notes.sync.last[&vault].detail.as_deref().unwrap().contains("Review Changes is running"));
        app.run(&ctx, Cmd::NotesSync);
        assert!(!app.notes.sync.running() && app.status.contains("Review Changes is running"), "{}", app.status);
    }

    #[test]
    fn saving_waits_while_git_rewrites_files() {
        let Some(f) = fixture("ui-hold") else { return };
        let ctx = Context::default();
        let mut app = open(&f, &ctx);
        enable(&mut app, 0);
        f.there_commits("vault/Ideas.md", "from there\n");
        write(&f.vault(), "Other.md", "mine\n");
        let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
        let (at_tx, at_rx) = std::sync::mpsc::channel::<()>();
        let go_rx = Mutex::new(go_rx);
        app.notes.sync.on_phase = Some(Arc::new(move |p| {
            if p == sync::Phase::Integrate {
                let _ = at_tx.send(());
                let _ = go_rx.lock().unwrap().recv();
            }
        }));
        app.run(&ctx, Cmd::NotesSync);
        at_rx.recv_timeout(Duration::from_secs(30)).expect("reached the rebase");
        // A checkout removes a file before writing it again: a tab looking
        // at that moment mustn't take it for deleted.
        let (plan, aside) = (f.vault().join("Plan.md"), f.base.join("Plan.md.aside"));
        std::fs::rename(&plan, &aside).unwrap();
        assert!(!app.docs[app.active].check_disk());
        assert!(!app.docs[app.active].deleted && !app.docs[app.active].is_dirty(), "not taken for deleted");
        std::fs::rename(&aside, &plan).unwrap();
        type_text(&mut app, &ctx, "typed meanwhile ");
        assert!(!app.save(app.active, false), "held while Git rewrites files");
        assert!(app.status.contains("Git sync is updating files here"), "{}", app.status);
        assert!(app.docs[app.active].is_dirty(), "the text stays in the tab");
        go_tx.send(()).unwrap();
        finish(&mut app, &ctx);
        assert!(app.save(app.active, false), "saves again once it's done");
        assert!(read(&f.vault(), "Plan.md").starts_with("typed meanwhile # Plan"));
    }

    #[test]
    fn conflicts_are_listed_compared_with_the_remote_and_settled() {
        let Some(f) = fixture("ui-conflict") else { return };
        let ctx = Context::default();
        let mut app = open(&f, &ctx);
        enable(&mut app, 0);
        f.there_commits("vault/Plan.md", "# Plan\n\nline one\nTHEIRS\nline three\n");
        write(&f.vault(), "Plan.md", "# Plan\n\nline one\nMINE\nline three\n");
        for _ in 0..8 {
            frame(&mut app, &ctx, vec![]);
        }
        app.docs[app.active].check_disk();
        let before = read(&f.vault(), "Plan.md");
        app.run(&ctx, Cmd::NotesSync);
        let out = finish(&mut app, &ctx);
        let out = if shows(&out, "Notes didn't sync") { out } else { frame(&mut app, &ctx, vec![]) };
        assert!(app.status.starts_with("Notes didn't sync: 1 note changed here and on origin/main"), "{}", app.status);
        assert!(shows(&out, "Notes didn't sync"), "{:?}", texts(&out));
        assert!(shows(&out, "Compare with origin/main"));
        assert!(shows(&out, "Plan.md"));
        assert!(shows(&out, "Sync: conflict"), "the status bar too");
        assert_eq!(read(&f.vault(), "Plan.md"), before, "the file is as it was");
        // Compare: the remote's version on the fixed side.
        app.compare_with_remote("vault/Plan.md");
        let c = app.docs[app.active].compare.as_ref().expect("a Compare tab");
        assert!(c.base_text().contains("THEIRS"));
        // The timer leaves a vault in conflict alone.
        app.settings.notes_git_sync.values_mut().for_each(|v| v.interval_minutes = 1);
        let vault = vault_key(&app);
        app.notes.sync.due.insert(vault.clone(), 0.0);
        app.notes.sync.last_check = 0.0;
        frame(&mut app, &ctx, vec![]);
        assert_eq!(app.notes.sync.timer_starts, 0);
        // Bring their line over, save, keep my versions.
        let merged = "# Plan\n\nline one\nMINE\nTHEIRS\nline three\n";
        std::fs::write(f.vault().join("Plan.md"), merged).unwrap();
        for i in 0..app.docs.len() {
            app.docs[i].check_disk();
        }
        app.notes.sync.show_conflict = true;
        app.keep_my_versions(&ctx);
        let out = finish(&mut app, &ctx);
        assert!(app.status.starts_with("Synced notes"), "{}", app.status);
        assert!(app.notes.sync.conflict.is_none() && !shows(&out, "Notes didn't sync"));
        assert_eq!(f.remote_text("vault/Plan.md"), merged.trim_end());
        assert_eq!(read(&f.vault(), "Plan.md"), merged);
    }
}
