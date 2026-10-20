//! AI helpers, the app half: the commands, the consent question before
//! anything goes to a remote provider, the answer panel at the bottom of the
//! window (streamed, rendered as Markdown, cancellable), and the key and
//! Test Connection row in Settings ▸ AI Helpers. The work is in `crate::ai`;
//! nothing here waits on the network, git or the keychain except
//! `KeyStore::exists`, which never shows a prompt.
//!
//! A request goes through three steps: the prompt is built on a background
//! thread (`Preparing`), the user is asked if the provider is remote and not
//! yet allowed (`Asking`: nothing has been sent), then it's sent (`Running`).

use super::StrataApp;
use super::settings_ui::Change;
use crate::ai::keychain::{self, KeyStore};
use crate::ai::prompt::{Ask, Prepared, Task};
use crate::ai::{Event, Job, Provider, TestResult};
use crate::commands::Cmd;
use crate::markdown::{DocRender, TopBlock};
use egui::RichText;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::Instant;

enum Phase {
    Idle,
    Preparing { rx: Receiver<Result<Prepared, String>>, provider: Provider },
    /// The consent question is up. Nothing has been sent.
    Asking { prepared: Prepared, provider: Provider },
    Running { job: Job },
}

pub(super) struct AiState {
    pub visible: bool,
    keys: Arc<dyn KeyStore>,
    phase: Phase,
    title: String,
    /// "localhost:11434 · llama3.2 · local".
    provider_label: String,
    notes: Vec<String>,
    answer: String,
    /// `answer` parsed, and how much of it that was.
    blocks: Vec<TopBlock>,
    parsed: usize,
    render: DocRender,
    error: Option<String>,
    /// Shown after the answer (cut off, refused).
    end_note: Option<String>,
    started: Option<Instant>,
    took: Option<f64>,
    test: Option<Receiver<TestResult>>,
    pub test_result: Option<TestResult>,
    /// Whether a key is saved for this keychain account (looked up when it changes).
    key_cache: Option<(String, Result<bool, String>)>,
    /// Test Connection is asking before it sends a key to a remote provider.
    test_confirm: Option<Provider>,
    /// Tests: treat the (local) test server as remote, and run this program as the command-line tool.
    #[cfg(test)]
    pub remote_for_tests: bool,
    #[cfg(test)]
    pub program_for_tests: Option<PathBuf>,
    #[cfg(test)]
    pub shown_host_for_tests: Option<String>,
}

impl Default for AiState {
    fn default() -> Self {
        // Tests never touch the real keychain (see keychain's gated test).
        #[cfg(test)]
        let keys: Arc<dyn KeyStore> = Arc::new(keychain::Memory::default());
        #[cfg(not(test))]
        let keys: Arc<dyn KeyStore> = Arc::new(keychain::System);
        Self {
            visible: false,
            keys,
            phase: Phase::Idle,
            title: String::new(),
            provider_label: String::new(),
            notes: Vec::new(),
            answer: String::new(),
            blocks: Vec::new(),
            parsed: 0,
            render: DocRender::default(),
            error: None,
            end_note: None,
            started: None,
            took: None,
            test: None,
            test_result: None,
            key_cache: None,
            test_confirm: None,
            #[cfg(test)]
            remote_for_tests: false,
            #[cfg(test)]
            program_for_tests: None,
            #[cfg(test)]
            shown_host_for_tests: None,
        }
    }
}

impl AiState {
    pub fn busy(&self) -> bool {
        !matches!(self.phase, Phase::Idle)
    }

    #[cfg(test)]
    pub fn asking(&self) -> bool {
        matches!(self.phase, Phase::Asking { .. })
    }

    #[cfg(test)]
    pub fn keys(&self) -> Arc<dyn KeyStore> {
        self.keys.clone()
    }

    #[cfg(test)]
    pub fn answer(&self) -> (&str, Option<&str>, Option<&str>) {
        (&self.answer, self.error.as_deref(), self.end_note.as_deref())
    }

    fn clear(&mut self, title: String, provider_label: String) {
        self.title = title;
        self.provider_label = provider_label;
        self.notes.clear();
        self.answer.clear();
        self.blocks.clear();
        self.parsed = 0;
        self.error = None;
        self.end_note = None;
        self.started = None;
        self.took = None;
    }
}

/// What the Settings window shows in its AI row.
#[derive(Default)]
pub(super) struct AiLook {
    /// The provider's problem, if its settings don't work yet.
    pub problem: Option<String>,
    /// Where the key is kept for ("openrouter.ai"), when the provider takes one.
    pub key_for: Option<String>,
    pub key_saved: Option<Result<bool, String>>,
    pub testing: bool,
    pub result: Option<TestResult>,
    pub cli: bool,
}

/// The key and Test Connection row in Settings ▸ AI Helpers.
pub(super) fn settings_row(draft: &mut String, ui: &mut egui::Ui, look: &AiLook, size: f32, changes: &mut Vec<Change>) {
    let dim = ui.visuals().weak_text_color();
    let small = crate::fonts::secondary_size(size);
    ui.add_space(14.0);
    ui.label(RichText::new("API key and connection").size(size).strong());
    ui.add_space(4.0);
    if let Some(p) = &look.problem {
        ui.add(egui::Label::new(RichText::new(p).size(small).color(ui.visuals().warn_fg_color)).wrap());
    } else if look.cli {
        ui.add(egui::Label::new(RichText::new("Command-line tools use their own login: Strata stores no key for them and never reads theirs.").size(small).color(dim)).wrap());
    } else if let Some(host) = &look.key_for {
        let state = match &look.key_saved {
            Some(Ok(true)) => format!("A key for {host} is saved in {}.", keychain::store_name()),
            Some(Ok(false)) => format!("No key saved for {host} (local servers usually need none)."),
            Some(Err(e)) => format!("Couldn't check {}: {e}", keychain::store_name()),
            None => String::new(),
        };
        ui.add(egui::Label::new(RichText::new(state).size(small).color(dim)).wrap());
        ui.horizontal(|ui| {
            let edit = egui::TextEdit::singleline(draft).password(true).hint_text("Paste a key").desired_width((ui.available_width() - size * 14.0).clamp(120.0, size * 22.0)).font(egui::FontId::proportional(size));
            let r = ui.add(edit);
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui.add_enabled(!draft.trim().is_empty(), egui::Button::new(RichText::new("Save Key").size(size))).clicked() || (enter && !draft.trim().is_empty()) {
                changes.push(Change::AiSaveKey(super::settings_ui::Secret(std::mem::take(draft).trim().to_owned())));
            }
            if ui.add_enabled(matches!(look.key_saved, Some(Ok(true))), egui::Button::new(RichText::new("Remove Key").size(size))).clicked() {
                changes.push(Change::AiRemoveKey);
            }
        });
        ui.add(egui::Label::new(RichText::new("The key is only ever sent to this address. It's kept out of settings.json and the log.").size(small).color(dim)).wrap());
    }
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.add_enabled(!look.testing && look.problem.is_none(), egui::Button::new(RichText::new("Test Connection").size(size))).on_hover_text("Ask the provider for its models (sends none of your text)").clicked() {
            changes.push(Change::AiTest);
        }
        if look.testing {
            ui.add(egui::Spinner::new().size(size));
        }
    });
    match &look.result {
        Some(Ok(m)) => {
            ui.add(egui::Label::new(RichText::new(crate::hidden::plain(m).into_owned()).size(small)).wrap());
        }
        Some(Err(e)) => {
            ui.add(egui::Label::new(RichText::new(e).size(small).color(ui.visuals().error_fg_color)).wrap());
        }
        None => {}
    }
    ui.add_space(8.0);
}

impl StrataApp {
    pub(super) fn ai_command(&mut self, ctx: &egui::Context, cmd: Cmd) {
        match cmd {
            Cmd::AiCancel => self.ai_cancel(),
            Cmd::AiTogglePanel => self.ai.visible = !self.ai.visible,
            Cmd::AiSettings => self.ai_open_settings(),
            Cmd::AiTestConnection => self.ai_test(ctx),
            Cmd::AiForgetAllowed => {
                self.status = match crate::ai::consent::Consent::new(self.config.as_deref()).forget_all() {
                    Ok(0) => "No providers were allowed: each remote one asks before its first request".into(),
                    Ok(n) => format!("Forgot {n} allowed provider{}: each asks again before its next request", if n == 1 { "" } else { "s" }),
                    Err(e) => e,
                }
            }
            Cmd::AiSummarizeChanges => self.ai_changes(ctx, Ask::Summarize),
            Cmd::AiFlagRisks => self.ai_changes(ctx, Ask::Risks),
            Cmd::AiDraftCommitMessage => self.ai_changes(ctx, Ask::CommitMessage),
            Cmd::AiSummarizeFile => self.ai_file(ctx),
            Cmd::AiExplainChange => self.ai_hunk(ctx),
            Cmd::AiExplainSelection => self.ai_selection(ctx),
            _ => {}
        }
    }

    pub(super) fn ai_open_settings(&mut self) {
        self.open_settings_window();
        self.settings_ui.show_category(crate::settings_meta::Category::Ai);
    }

    /// The provider the settings describe.
    fn ai_resolve(&self) -> Result<Provider, String> {
        #[allow(unused_mut)]
        let mut p = Provider::from_settings(&self.settings)?;
        #[cfg(test)]
        {
            p.remote_for_tests = self.ai.remote_for_tests;
            p.program_for_tests = self.ai.program_for_tests.clone();
            p.shown_host_for_tests = self.ai.shown_host_for_tests.clone();
        }
        Ok(p)
    }

    /// The provider to use, or why there isn't one (said in the status bar).
    fn ai_provider(&mut self) -> Option<Provider> {
        if !self.settings.ai_enabled {
            self.status = "AI helpers are off: turn them on in Settings ▸ AI Helpers".into();
            self.ai_open_settings();
            return None;
        }
        match self.ai_resolve() {
            Ok(p) => Some(p),
            Err(e) => {
                self.status = e;
                None
            }
        }
    }

    /// Unsaved text of open tabs, by path (the rest is read from disk).
    fn ai_open_texts(&mut self) -> HashMap<PathBuf, String> {
        let mut out = HashMap::new();
        for d in &mut self.docs {
            if d.is_dirty()
                && let Some(p) = d.path.clone()
            {
                out.insert(p, d.editor.text_snapshot().to_string());
            }
        }
        out
    }

    fn ai_changes(&mut self, ctx: &egui::Context, ask: Ask) {
        let Some(provider) = self.ai_provider() else { return };
        let Some(folder) = self.review_folder() else {
            self.status = "Open a folder or a file in a Git repository first".into();
            return;
        };
        let (base, scan) = if ask == Ask::CommitMessage {
            // A commit message is about everything since the last commit,
            // whatever a review started from.
            (crate::review::Base::Head, (!self.review.reviewing()).then(|| self.review.scan.clone()).flatten())
        } else {
            (self.review.base.clone(), self.review.scan.clone())
        };
        let open = self.ai_open_texts();
        self.ai_prepare(ctx, provider, Task::Changes { ask, folder, base, scan, open });
    }

    /// The current review tab: (file, relative path).
    fn ai_review_tab(&mut self) -> Option<(PathBuf, String)> {
        let path = self.docs.get(self.active)?.compare.as_ref()?.review.clone()?;
        let rel = self.review.scan.as_ref().and_then(|s| s.files.iter().find(|f| f.path == path).map(|f| f.rel.clone())).unwrap_or_else(|| path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned()));
        Some((path, rel))
    }

    fn ai_file(&mut self, ctx: &egui::Context) {
        let Some((path, rel)) = self.ai_review_tab() else {
            self.status = "Open a file's changes in Review Changes first (Review: Next Change), or use AI: Summarize Changes for all of them".into();
            return;
        };
        let Some(provider) = self.ai_provider() else { return };
        let d = &mut self.docs[self.active];
        let base = d.compare.as_ref().expect("review tab").base_text().to_owned();
        let current = d.editor.text_snapshot().to_string();
        self.ai_prepare(ctx, provider, Task::File { ask: Ask::Summarize, path, rel, base, current });
    }

    fn ai_hunk(&mut self, ctx: &egui::Context) {
        let Some((path, rel)) = self.ai_review_tab() else {
            self.status = "Open a file's changes in Review Changes first (Review: Next Change)".into();
            return;
        };
        let d = &mut self.docs[self.active];
        let c = d.compare.as_mut().expect("review tab");
        c.refresh(&mut d.editor, 0.0, true);
        let Some(k) = c.hunk_at_caret(&d.editor) else {
            self.status = "Put the caret on a change first (Review: Next Change)".into();
            return;
        };
        let hunk = c.hunks()[k].clone();
        let base = c.base_text().to_owned();
        let current = d.editor.text_snapshot().to_string();
        let Some(provider) = self.ai_provider() else { return };
        self.ai_prepare(ctx, provider, Task::Hunk { path, rel, base, current, hunk });
    }

    fn ai_selection(&mut self, ctx: &egui::Context) {
        let d = &self.docs[self.active];
        let ranges: Vec<_> = d.editor.selections().iter().filter(|s| !s.is_empty()).map(|s| s.range()).collect();
        if ranges.is_empty() {
            self.status = "Select some code first".into();
            return;
        }
        let text = ranges.into_iter().map(|r| d.editor.buffer.slice(r)).collect::<Vec<_>>().join("\n…\n");
        let (name, syntax, path) = (d.name(), d.highlighter.syntax_name().to_owned(), d.path.clone());
        let Some(provider) = self.ai_provider() else { return };
        self.ai_prepare(ctx, provider, Task::Selection { name, syntax, text, path });
    }

    /// Build the prompt off the UI thread; `ai_frame` takes it from there.
    fn ai_prepare(&mut self, ctx: &egui::Context, provider: Provider, task: Task) {
        self.ai_stop();
        let limit = (self.settings.ai_max_prompt_kb.clamp(8, 4096) as usize) << 10;
        let (tx, rx) = std::sync::mpsc::channel();
        let wake = ctx.clone();
        let spawned = std::thread::Builder::new().name("ai-prepare".into()).spawn(move || {
            let _ = tx.send(crate::ai::prompt::prepare(task, limit));
            wake.request_repaint();
        });
        if let Err(e) = spawned {
            self.status = format!("Couldn't start: {e}");
            return;
        }
        self.ai.clear("Preparing…".into(), provider.label());
        self.ai.phase = Phase::Preparing { rx, provider };
        self.ai.visible = true;
    }

    /// Send what was prepared; the only place a request starts.
    fn ai_send(&mut self, ctx: Option<&egui::Context>, prepared: Prepared, provider: Provider) {
        self.ai.notes = prepared.notes;
        self.ai.title = prepared.title;
        self.ai.provider_label = provider.label();
        self.ai.started = Some(Instant::now());
        let job = crate::ai::start(provider, prepared.prompt, self.ai.keys.clone(), ctx.cloned());
        self.ai.phase = Phase::Running { job };
        self.ai.visible = true;
    }

    fn ai_stop(&mut self) {
        // Dropping a job cancels it.
        self.ai.phase = Phase::Idle;
    }

    pub(super) fn ai_cancel(&mut self) {
        match &self.ai.phase {
            Phase::Idle => self.status = "No AI request is running".into(),
            Phase::Running { job } => job.cancel(), // its last event says so
            Phase::Preparing { .. } | Phase::Asking { .. } => {
                self.ai.phase = Phase::Idle;
                self.ai.error = Some("Cancelled: nothing was sent.".into());
            }
        }
    }

    /// The consent question was answered.
    fn ai_answer(&mut self, ctx: &egui::Context, allow: bool, always: bool) {
        let Phase::Asking { prepared, provider } = std::mem::replace(&mut self.ai.phase, Phase::Idle) else { return };
        if !allow {
            self.ai.error = Some("Cancelled: nothing was sent.".into());
            return;
        }
        if always && let Err(e) = crate::ai::consent::Consent::new(self.config.as_deref()).allow(&provider.consent_id()) {
            self.status = format!("Sent this once; {e}");
        }
        self.ai_send(Some(ctx), prepared, provider);
    }

    /// Each frame: take what arrived, ask if needed.
    pub(super) fn ai_frame(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.ai.test
            && let Ok(r) = rx.try_recv()
        {
            self.ai.test = None;
            self.ai.test_result = Some(r);
        }
        if self.settings_ui.open {
            self.ai_refresh_key();
        }
        self.ai_poll(Some(ctx));
        if let Phase::Running { .. } | Phase::Preparing { .. } = self.ai.phase {
            // The elapsed time and the spinner.
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
        self.ai_consent_ui(ctx);
        self.ai_test_confirm_ui(ctx);
    }

    /// Take finished preparations and streamed text. True if something arrived.
    pub(super) fn ai_poll(&mut self, ctx: Option<&egui::Context>) -> bool {
        let mut got = false;
        if let Phase::Preparing { rx, .. } = &self.ai.phase
            && let Ok(result) = rx.try_recv()
        {
            got = true;
            let Phase::Preparing { provider, .. } = std::mem::replace(&mut self.ai.phase, Phase::Idle) else { unreachable!() };
            match result {
                Err(e) => {
                    self.ai.title = "Nothing sent".into();
                    self.ai.error = Some(e);
                }
                Ok(prepared) => {
                    let allowed = provider.is_local() || crate::ai::consent::Consent::new(self.config.as_deref()).allowed(&provider.consent_id());
                    if allowed {
                        self.ai_send(ctx, prepared, provider);
                    } else {
                        self.ai.title = prepared.title.clone();
                        self.ai.notes = prepared.notes.clone();
                        self.ai.phase = Phase::Asking { prepared, provider };
                    }
                }
            }
        }
        let mut finished = false;
        if let Phase::Running { job } = &self.ai.phase {
            while let Ok(e) = job.rx.try_recv() {
                got = true;
                match e {
                    Event::Text(t) => self.ai.answer.push_str(&t),
                    Event::Done(note) => {
                        self.ai.end_note = note;
                        finished = true;
                    }
                    Event::Failed(f) => {
                        self.ai.error = Some(f);
                        finished = true;
                    }
                }
                if finished {
                    break;
                }
            }
        }
        if finished {
            self.ai.took = self.ai.started.map(|s| s.elapsed().as_secs_f64());
            self.ai.phase = Phase::Idle;
        }
        got
    }

    fn ai_consent_ui(&mut self, ctx: &egui::Context) {
        let Phase::Asking { prepared, provider } = &self.ai.phase else { return };
        let (mut once, mut always, mut cancel) = (false, false, false);
        let size = self.ui_size();
        let full = format!("{}\n\n{}", prepared.prompt.system, prepared.prompt.user);
        crate::fonts::ensure_cjk(ctx, &full);
        let screen = ctx.content_rect();
        egui::Modal::new(egui::Id::new("ai-consent")).show(ctx, |ui| {
            ui.set_width((screen.width() - 80.0).clamp(320.0, 720.0));
            ui.label(RichText::new(format!("Send this to {}?", provider.destination())).strong().size(size * 1.1));
            ui.add_space(4.0);
            let model = if provider.model.is_empty() { String::new() } else { format!(" for {}", provider.model) };
            ui.add(egui::Label::new(RichText::new(format!("It isn't on this computer or your local network. This is everything Strata will send{model} ({}):", crate::binary::size_label(full.len() as u64))).size(crate::fonts::secondary_size(size))).wrap());
            for n in &prepared.notes {
                ui.add(egui::Label::new(RichText::new(n).size(crate::fonts::secondary_size(size)).color(ui.visuals().warn_fg_color)).wrap());
            }
            ui.add_space(6.0);
            egui::Frame::new().fill(ui.visuals().extreme_bg_color).corner_radius(4).inner_margin(8).show(ui, |ui| {
                egui::ScrollArea::vertical().max_height((screen.height() * 0.5).max(160.0)).auto_shrink([false, true]).show(ui, |ui| {
                    let mut shown = full.as_str();
                    ui.add(egui::TextEdit::multiline(&mut shown).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));
                });
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                once = ui.button("Allow Once").clicked();
                always = ui.button("Always for This Provider").on_hover_text("Don't ask again for this provider (AI: Forget Allowed Providers undoes it)").clicked();
                cancel = ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape));
            });
        });
        if once || always {
            self.ai_answer(ctx, true, always);
        } else if cancel {
            self.ai_answer(ctx, false, false);
        }
    }

    /// The answer panel, at the bottom like build results.
    pub(super) fn ai_panel(&mut self, ui: &mut egui::Ui) {
        if !self.ai.visible || self.distraction.is_some() {
            return;
        }
        let id = self.link.id("ai-panel");
        let size = self.settings.font_size.clamp(10.0, 20.0);
        let hl = &self.docs[self.active].highlighter;
        let a = &mut self.ai;
        let mut act = None;
        egui::Panel::bottom(id).resizable(true).default_size(220.0).min_size(80.0).show(ui, |ui| act = panel_ui(a, ui, hl, size));
        match act {
            Some(PanelAct::Cancel) => self.ai_cancel(),
            Some(PanelAct::Close) => {
                self.ai.visible = false;
                if let Phase::Running { job } = &self.ai.phase {
                    job.cancel();
                }
            }
            Some(PanelAct::NewTab) => self.ai_open_in_tab(),
            Some(PanelAct::Copy) => {
                ui.ctx().copy_text(self.ai.answer.clone());
                self.status = "Copied the answer".into();
            }
            Some(PanelAct::Link(url)) => ui.ctx().open_url(egui::OpenUrl::new_tab(url)),
            None => {}
        }
    }


    /// The answer in a new, untitled tab (for a commit message: to copy into
    /// your commit tool; Strata doesn't commit).
    pub(super) fn ai_open_in_tab(&mut self) {
        let text = self.ai.answer.clone();
        self.push_doc(crate::document::Document::untitled());
        let i = self.active;
        self.docs[i].editor.insert_text(&text);
        self.docs[i].highlighter.set_syntax("Markdown");
    }

    pub(super) fn ai_test(&mut self, ctx: &egui::Context) {
        if !self.settings.ai_enabled {
            self.status = "AI helpers are off: turn them on in Settings ▸ AI Helpers first".into();
            self.ai_open_settings();
            return;
        }
        let provider = match self.ai_resolve() {
            Ok(p) => p,
            Err(e) => {
                self.ai.test_result = Some(Err(e.clone()));
                self.status = e;
                return;
            }
        };
        self.ai.test_result = None;
        // Testing a remote provider sends the saved key there: say so first,
        // once per provider (or not at all once its prompts are allowed).
        let consent = crate::ai::consent::Consent::new(self.config.as_deref());
        let key_saved = provider.key_account().is_some_and(|a| self.ai.keys.exists(&a).unwrap_or(true));
        if key_saved && !provider.is_local() && !consent.allowed(&provider.consent_id()) && !consent.allowed(&key_test_id(&provider)) {
            self.ai.test_confirm = Some(provider);
            return;
        }
        self.ai_start_test(ctx, provider);
    }

    fn ai_start_test(&mut self, ctx: &egui::Context, provider: Provider) {
        self.ai.test = Some(crate::ai::test_connection(provider, self.ai.keys.clone(), Some(ctx.clone())));
        self.status = "Testing the connection…".into();
    }

    /// The Test Connection question was answered.
    pub(super) fn ai_test_answer(&mut self, ctx: &egui::Context, send: bool) {
        let Some(provider) = self.ai.test_confirm.take() else { return };
        if !send {
            self.status = "Test cancelled: nothing was sent".into();
            return;
        }
        if let Err(e) = crate::ai::consent::Consent::new(self.config.as_deref()).allow(&key_test_id(&provider)) {
            self.status = e;
        }
        self.ai_start_test(ctx, provider);
    }

    fn ai_test_confirm_ui(&mut self, ctx: &egui::Context) {
        let Some(provider) = &self.ai.test_confirm else { return };
        let (mut send, mut cancel) = (false, false);
        let host = provider.shown_host();
        egui::Modal::new(egui::Id::new("ai-test-confirm")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.label(RichText::new(format!("Test Connection sends your API key to {host} to check it.")).strong());
            ui.add_space(4.0);
            ui.label(RichText::new("None of your text is sent. Strata won't ask again for this provider.").small().weak());
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                send = ui.button("Send Key and Test").clicked();
                cancel = ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape));
            });
        });
        if send {
            self.ai_test_answer(ctx, true);
        } else if cancel {
            self.ai_test_answer(ctx, false);
        }
    }

    /// The keychain account the current settings use, and where it's for.
    fn ai_account(&self) -> Option<(String, String)> {
        let p = Provider::from_settings(&self.settings).ok()?;
        Some((p.key_account()?, p.endpoint.host_label()))
    }

    fn ai_refresh_key(&mut self) {
        let Some((account, _)) = self.ai_account() else { return };
        if self.ai.key_cache.as_ref().is_some_and(|(a, _)| *a == account) {
            return;
        }
        let saved = self.ai.keys.exists(&account);
        self.ai.key_cache = Some((account, saved));
    }

    /// For the Settings window's AI row.
    pub(super) fn ai_look(&self) -> AiLook {
        let provider = Provider::from_settings(&self.settings);
        let account = self.ai_account();
        AiLook {
            problem: provider.as_ref().err().cloned(),
            cli: provider.as_ref().is_ok_and(|p| p.kind.is_cli()),
            key_saved: account.as_ref().and_then(|(a, _)| self.ai.key_cache.as_ref().filter(|(c, _)| c == a).map(|(_, s)| s.clone())),
            key_for: account.map(|(_, host)| host),
            testing: self.ai.test.is_some(),
            result: self.ai.test_result.clone(),
        }
    }

    pub(super) fn ai_save_key(&mut self, key: &str) {
        let Some((account, host)) = self.ai_account() else { return };
        self.status = match self.ai.keys.set(&account, key) {
            Ok(()) => format!("Saved the key for {host} in {}", keychain::store_name()),
            Err(e) => format!("Couldn't save the key: {e}"),
        };
        self.ai.key_cache = None;
        self.ai.test_result = None;
    }

    pub(super) fn ai_remove_key(&mut self) {
        let Some((account, host)) = self.ai_account() else { return };
        self.status = match self.ai.keys.delete(&account) {
            Ok(()) => format!("Removed the key for {host}"),
            Err(e) => format!("Couldn't remove the key: {e}"),
        };
        self.ai.key_cache = None;
        self.ai.test_result = None;
    }
}

/// The panel's inside: header, notes, the answer and how it ended.
fn panel_ui(a: &mut AiState, ui: &mut egui::Ui, hl: &crate::highlight::Highlighter, size: f32) -> Option<PanelAct> {
    let mut act = None;
    let running = matches!(a.phase, Phase::Running { .. } | Phase::Preparing { .. });
    ui.horizontal(|ui| {
        if running {
            ui.add(egui::Spinner::new().size(12.0));
        }
        ui.label(RichText::new(crate::hidden::plain(if a.title.is_empty() { "AI" } else { &a.title }).into_owned()).strong());
        if !a.provider_label.is_empty() {
            ui.label(RichText::new(&a.provider_label).weak().small());
        }
        match (a.started, a.took) {
            (_, Some(t)) => {
                ui.label(RichText::new(format!("{t:.1}s")).weak().small());
            }
            (Some(s), None) if running => {
                ui.label(RichText::new(format!("{:.1}s", s.elapsed().as_secs_f64())).weak().small());
            }
            _ => {}
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("×").on_hover_text("Hide the panel (stops a request)").clicked() {
                act = Some(PanelAct::Close);
            }
            if ui.add_enabled(running, egui::Button::new("Cancel").small()).clicked() {
                act = Some(PanelAct::Cancel);
            }
            let has = !a.answer.is_empty();
            if ui.add_enabled(has, egui::Button::new("Copy").small()).on_hover_text("Copy the answer").clicked() {
                act = Some(PanelAct::Copy);
            }
            if ui.add_enabled(has && !running, egui::Button::new("Open in New Tab").small()).clicked() {
                act = Some(PanelAct::NewTab);
            }
        });
    });
    for n in &a.notes {
        ui.label(RichText::new(n).small().color(ui.visuals().warn_fg_color));
    }
    ui.separator();
    if a.parsed != a.answer.len() {
        a.blocks = crate::markdown::parse(&a.answer);
        a.parsed = a.answer.len();
    }
    let mut scroll = egui::ScrollArea::vertical().id_salt("ai-answer").auto_shrink(false);
    if running {
        scroll = scroll.stick_to_bottom(true);
    }
    scroll.show(ui, |ui| {
        if matches!(a.phase, Phase::Preparing { .. }) {
            ui.label(RichText::new("Gathering what to send…").weak());
        } else if matches!(a.phase, Phase::Asking { .. }) {
            ui.label(RichText::new("Waiting for your answer: nothing has been sent.").weak());
        } else if running && a.answer.is_empty() {
            ui.label(RichText::new("Waiting for the answer…").weak());
        }
        if !a.blocks.is_empty() {
            let frame = a.render.show(ui, hl, size, &a.blocks);
            if let Some(url) = frame.clicked
                && (url.starts_with("https://") || url.starts_with("http://"))
            {
                act = Some(PanelAct::Link(url));
            }
        }
        if let Some(n) = &a.end_note {
            ui.label(RichText::new(n).color(ui.visuals().warn_fg_color));
        }
        if let Some(e) = &a.error {
            ui.add(egui::Label::new(RichText::new(e).color(ui.visuals().error_fg_color)).wrap());
        }
    });
    act
}

/// What the Test Connection answer is remembered under.
fn key_test_id(p: &Provider) -> String {
    format!("key-test {}", p.consent_id())
}

enum PanelAct {
    Cancel,
    Close,
    Copy,
    NewTab,
    Link(String),
}

#[cfg(test)]
mod tests;
