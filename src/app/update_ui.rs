//! Update notices: asking once whether to check automatically, Help ▸ Check
//! for Updates…, and the corner card that offers a new version (with its
//! release notes), shows the download, and relaunches. The work itself is
//! in `crate::update`; nothing here blocks the window.

use super::StrataApp;
use crate::markdown::{DocRender, TopBlock};
use crate::update::{self, Mode, Status, Updater};
use egui::RichText;
use serde_json::Value;
use std::sync::Arc;

pub struct UpdateUi {
    updater: Arc<Updater>,
    /// Release notes parsed for the notice: (version, blocks).
    notes: Option<(String, Vec<TopBlock>)>,
    render: DocRender,
    /// When this window last looked at whether an automatic check is due.
    last_poll: f64,
    /// The launch check has been considered.
    polled: bool,
    /// Quit was asked for to relaunch: (time, frames drawn since). Frames
    /// still coming a while later mean the quit was cancelled.
    quitting: Option<(f64, u32)>,
    /// "Installed" was put away with OK.
    installed_hidden: bool,
    /// The ask card was answered or put away in this window.
    ask_done: bool,
    /// Tests and screenshots: show the ask card even under `cfg(test)`.
    pub force_ask: bool,
}

impl Default for UpdateUi {
    fn default() -> Self {
        // Tests never touch the network or the shared updater.
        #[cfg(test)]
        let updater = Updater::for_tests();
        #[cfg(not(test))]
        let updater = Updater::global();
        Self { updater, notes: None, render: DocRender::default(), last_poll: f64::NEG_INFINITY, polled: false, quitting: None, installed_hidden: false, ask_done: false, force_ask: false }
    }
}

impl UpdateUi {
    #[cfg(test)]
    pub fn updater(&self) -> &Arc<Updater> {
        &self.updater
    }
}

/// What a click asked for, carried out after drawing.
enum Act {
    TurnOn,
    NotNow,
    Install,
    Later,
    Skip,
    ReleasePage,
    Cancel,
    Retry,
    Relaunch,
    HideInstalled,
    Close,
    Link(String),
}

impl StrataApp {
    fn prerelease_channel(&self) -> bool {
        self.settings.update_channel.trim().eq_ignore_ascii_case("prerelease")
    }

    /// Automatic things (the first frame's tidy-up, automatic checks, the ask
    /// card) happen for real windows only: not in tests or screenshots.
    fn update_automatic(&self) -> bool {
        self.config.is_some() && !cfg!(test) && crate::dev::var("STRATA_SCREENSHOT").is_none()
    }

    /// Help ▸ Check for Updates…: always works, whatever `update_check` says.
    pub(super) fn check_for_updates(&mut self, ctx: &egui::Context) {
        self.update_start(ctx);
        let me = self.link.id("update-notice").value();
        let now = ctx.input(|i| i.time);
        let u = self.update.updater.clone();
        u.notice_owner(me, self.link.index == 0, now, true);
        self.update.installed_hidden = false;
        // Already checking, downloading or installed: the card shows where it is.
        if !u.status().busy() && !matches!(u.status(), Status::Installed { .. }) {
            u.check(self.prerelease_channel(), true);
        }
    }

    /// Once per process: tell the updater where its state is and how to
    /// repaint, finish a just-installed update, and show its note.
    fn update_start(&mut self, ctx: &egui::Context) {
        let u = self.update.updater.clone();
        if !u.first_start() {
            return;
        }
        let repaint = ctx.clone();
        u.set_waker(Arc::new(move || repaint.request_repaint()));
        if self.update_automatic() {
            u.set_config(self.config.clone());
            if let Some(note) = u.startup() {
                u.set_status_message(note);
            }
        }
    }

    /// Each frame: automatic checks, the ask card, and the notice.
    pub(super) fn update_ui(&mut self, ctx: &egui::Context) {
        self.update_start(ctx);
        let now = ctx.input(|i| i.time);
        let first = self.link.index == 0;
        let u = self.update.updater.clone();
        let mode = Mode::parse(&self.settings.update_check);
        if first && self.update_automatic() && mode == Mode::Auto {
            if now - self.update.last_poll > 60.0 {
                let launch = !self.update.polled;
                self.update.last_poll = now;
                self.update.polled = true;
                let seed = self.config.as_deref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
                if u.auto_check_due(mode, launch, crate::update::state::interval_secs(self.settings.update_check_interval_hours) + crate::update::state::jitter_secs(&seed)) {
                    u.check(self.prerelease_channel(), false);
                }
            }
            // An idle window draws no frames: wake once a minute to look.
            ctx.request_repaint_after(std::time::Duration::from_secs(60));
        }
        self.update_relaunch(ctx, now);
        // The crash notice has the corner first.
        if self.crash_notice.is_some() {
            return;
        }
        let me = self.link.id("update-notice").value();
        let status = u.status();
        let shows = match &status {
            Status::Idle | Status::Checking { manual: false } | Status::UpToDate { manual: false } => false,
            Status::Installed { .. } => !self.update.installed_hidden,
            Status::Failed { manual, .. } => *manual,
            _ => true,
        };
        if shows {
            if u.notice_owner(me, first, now, false) {
                self.update_notice(ctx, &status);
            }
        } else if first && mode == Mode::Ask && !self.update.ask_done && !self.first_run.open && (self.update_automatic() || self.update.force_ask) {
            self.update_ask(ctx);
        }
    }

    /// Installed: quit to relaunch (once), and notice when the quit didn't
    /// happen (a Cancel when asked about unsaved changes).
    fn update_relaunch(&mut self, ctx: &egui::Context, now: f64) {
        let u = self.update.updater.clone();
        let Status::Installed { plan, .. } = u.status() else {
            self.update.quitting = None;
            return;
        };
        match &mut self.update.quitting {
            // The window showing the notice asks to quit (every window goes).
            None if u.notice_owner(self.link.id("update-notice").value(), self.link.index == 0, now, false) => {
                self.update.quitting = Some((now, 0));
                u.set_relaunch(Some(plan));
                self.quit(ctx);
            }
            Some((at, frames)) => {
                *frames += 1;
                if *frames == 10 && now - *at > 2.0 {
                    // Still here: not quitting after all, so don't relaunch at a later quit.
                    u.set_relaunch(None);
                }
            }
            None => {}
        }
    }

    fn quit_was_cancelled(&self) -> bool {
        self.update.quitting.is_some_and(|(_, frames)| frames >= 10)
    }

    /// "Check for updates automatically?", once, in the corner.
    fn update_ask(&mut self, ctx: &egui::Context) {
        let mut act = None;
        notice_window(ctx, self.link.id("update-ask"), |ui| {
            ui.strong("Check for updates automatically?");
            ui.label("When Strata opens and every few hours while it's open, it will ask GitHub whether a new version is out. No other data is sent.");
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("Turn On").clicked() {
                    act = Some(Act::TurnOn);
                }
                if ui.button("Not Now").clicked() {
                    act = Some(Act::NotNow);
                }
            });
            ui.label(RichText::new("Help ▸ Check for Updates… always works, and Settings can change this.").small().weak());
        });
        if let Some(act) = act {
            self.update_act(ctx, act);
        }
    }

    fn update_notice(&mut self, ctx: &egui::Context, status: &Status) {
        let mut act = None;
        let current = self.update.updater.current().clone();
        let size = self.ui_size();
        // Release notes, parsed once per version.
        if let Status::Available { release, .. } = status {
            let v = release.version().to_string();
            if self.update.notes.as_ref().is_none_or(|(n, _)| *n != v) {
                self.update.notes = Some((v, crate::markdown::parse(&release.manifest.notes)));
            }
        }
        let cancelled = self.quit_was_cancelled();
        let hl = &self.docs[self.active].highlighter;
        let (notes, render) = (&self.update.notes, &mut self.update.render);
        notice_window(ctx, self.link.id("update-notice"), |ui| match status {
            Status::Checking { .. } => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.strong("Checking for updates…");
                });
                if ui.button("Cancel").clicked() {
                    act = Some(Act::Cancel);
                }
            }
            Status::UpToDate { .. } => {
                ui.strong("Strata is up to date");
                ui.label(format!("{current} is the newest version."));
                if ui.button("OK").clicked() {
                    act = Some(Act::Close);
                }
            }
            Status::Available { release, .. } => {
                ui.strong(RichText::new(format!("Strata {} is available", release.version())).size(size * 1.15));
                let date = release.manifest.pub_date.as_deref().and_then(|d| d.get(..10)).map(|d| format!("Released {d}. ")).unwrap_or_default();
                ui.label(RichText::new(format!("{date}You have {current}.")).weak());
                if let Some((_, blocks)) = notes.as_ref().filter(|(_, b)| !b.is_empty()) {
                    ui.add_space(2.0);
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        egui::ScrollArea::vertical().max_height(220.0).auto_shrink([false, true]).show(ui, |ui| {
                            if let Some(url) = render.show(ui, hl, crate::fonts::secondary_size(size), blocks).clicked {
                                act = Some(Act::Link(url));
                            }
                        });
                    });
                }
                if let Err(why) = &release.installable {
                    ui.label(RichText::new(format!("{why}. Download it from the release page instead.").replace("..", ".")).small().weak());
                }
                ui.add_space(2.0);
                ui.horizontal_wrapped(|ui| {
                    let install = if release.installable.is_ok() { "Install and Relaunch" } else { "Download…" };
                    if ui.button(RichText::new(install).strong()).clicked() {
                        act = Some(Act::Install);
                    }
                    if ui.button("Later").clicked() {
                        act = Some(Act::Later);
                    }
                    if ui.button("Skip This Version").clicked() {
                        act = Some(Act::Skip);
                    }
                    if ui.button("Release Notes").on_hover_text("Open the release page on GitHub").clicked() {
                        act = Some(Act::ReleasePage);
                    }
                });
            }
            Status::Downloading { release, done, total } => {
                ui.strong(format!("Downloading Strata {}…", release.version()));
                let mb = |b: u64| b as f64 / 1_048_576.0;
                let frac = if *total > 0 { *done as f32 / *total as f32 } else { 0.0 };
                ui.add(egui::ProgressBar::new(frac).desired_width(360.0).text(format!("{:.1} of {:.1} MB", mb(*done), mb(*total))));
                if ui.button("Cancel").clicked() {
                    act = Some(Act::Cancel);
                }
            }
            Status::Installing { release, step } => {
                ui.strong(format!("Installing Strata {}", release.version()));
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(*step);
                });
            }
            Status::Installed { release, .. } => {
                ui.strong(format!("Strata {} is installed", release.version()));
                if cancelled {
                    ui.label("It starts the next time you open Strata.");
                    ui.horizontal(|ui| {
                        if ui.button("Relaunch Now").clicked() {
                            act = Some(Act::Relaunch);
                        }
                        if ui.button("OK").clicked() {
                            act = Some(Act::HideInstalled);
                        }
                    });
                } else {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Relaunching…");
                    });
                }
            }
            Status::Failed { error, release, .. } => {
                ui.strong(if release.is_some() { "Couldn't install the update" } else { "Couldn't check for updates" });
                ui.label(error.as_str());
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Try Again").clicked() {
                        act = Some(Act::Retry);
                    }
                    if ui.button("Download in Browser").clicked() {
                        act = Some(Act::ReleasePage);
                    }
                    if ui.button("Close").clicked() {
                        act = Some(Act::Close);
                    }
                });
            }
            Status::Message(text) => {
                ui.label(text.as_str());
                if ui.button("OK").clicked() {
                    act = Some(Act::Close);
                }
            }
            Status::Idle => {}
        });
        if let Some(act) = act {
            self.update_act(ctx, act);
        }
    }

    fn update_act(&mut self, ctx: &egui::Context, act: Act) {
        let u = self.update.updater.clone();
        let status = u.status();
        let release = match &status {
            Status::Available { release, .. } | Status::Downloading { release, .. } | Status::Installing { release, .. } | Status::Installed { release, .. } => Some(release.clone()),
            Status::Failed { release, .. } => release.clone(),
            _ => None,
        };
        match act {
            Act::TurnOn | Act::NotNow => {
                self.update.ask_done = true;
                let on = matches!(act, Act::TurnOn);
                self.change_setting(ctx, "update_check", Value::from(if on { "auto" } else { "off" }), true);
                self.status = if on { "Strata will check for updates when it opens and every few hours".into() } else { "Strata won't check for updates (Help ▸ Check for Updates… still works)".into() };
            }
            Act::Install => match release {
                Some(r) if r.installable.is_ok() => u.install(),
                Some(r) => self.open_release_page(ctx, Some(r.version())),
                None => {}
            },
            Act::Later | Act::Close => u.dismiss(),
            Act::Skip => {
                if let Some(r) = release {
                    u.skip(r.version());
                    self.status = format!("Strata won't offer {} again (Help ▸ Check for Updates… still shows it)", r.version());
                }
            }
            Act::ReleasePage => self.open_release_page(ctx, release.as_ref().map(|r| r.version())),
            Act::Cancel => u.cancel(),
            Act::Retry => match release {
                Some(_) => u.install(),
                None => u.check(self.prerelease_channel(), true),
            },
            Act::Relaunch => {
                self.update.quitting = None;
                self.update.installed_hidden = false;
            }
            Act::HideInstalled => self.update.installed_hidden = true,
            Act::Link(url) => {
                if let Err(msg) = crate::links::open_web(ctx, &url) {
                    self.status = msg;
                }
            }
        }
    }

    fn open_release_page(&mut self, ctx: &egui::Context, version: Option<&update::Version>) {
        let url = version.map_or_else(update::manifest::releases_page, update::manifest::release_page);
        if let Err(msg) = crate::links::open_web(ctx, &url) {
            self.status = msg;
        }
    }
}

/// The corner card every update notice uses.
fn notice_window(ctx: &egui::Context, id: egui::Id, body: impl FnOnce(&mut egui::Ui)) {
    egui::Window::new("Strata update").id(id).anchor(egui::Align2::RIGHT_BOTTOM, [-16.0, -40.0]).collapsible(false).resizable(false).title_bar(false).show(ctx, |ui| {
        // Wide enough for the buttons on one row at larger interface sizes.
        let size = egui::TextStyle::Body.resolve(ui.style()).size;
        ui.set_max_width((size * 30.0).max(400.0));
        body(ui);
    });
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::commands::Cmd;
    use crate::update::{Manifest, Release};

    pub fn release(installable: Result<(), String>) -> Release {
        let feed = crate::update::manifest::tests::sample("1.4.0", &[("macos-universal", "https://github.com/DRAZY/strata/releases/download/v1.4.0/Strata-1.4.0-universal.app.zip")]);
        let mut manifest = Manifest::parse(feed.as_bytes()).unwrap();
        manifest.notes = "## What's new\n\n- **Auto-update**: Strata tells you when a new version is out and installs it for you.\n- Find history in the find bar.\n- Faster startup with large sessions.\n\n## Fixes\n\n- Unsaved tabs survive a crash during save.".into();
        Release { manifest, installable }
    }

    fn frame(app: &mut StrataApp, ctx: &egui::Context) {
        let raw = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0))), ..Default::default() };
        let mut out = ctx.run_ui(raw, |ui| eframe::App::ui(app, ui, &mut eframe::Frame::_new_kittest()));
        out.textures_delta.clear();
    }

    #[test]
    fn check_for_updates_is_a_command_in_the_help_menu() {
        assert_eq!(Cmd::CheckForUpdates.id(), "check_for_updates");
        assert!(Cmd::ALL.contains(&Cmd::CheckForUpdates));
        let ctx = egui::Context::default();
        let mut app = StrataApp::new(None, None);
        app.run(&ctx, Cmd::CheckForUpdates);
        // The test updater has no network: a manual check says so.
        let mut status = app.update.updater().status();
        for _ in 0..200 {
            if !status.busy() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
            status = app.update.updater().status();
        }
        assert!(matches!(status, Status::Failed { manual: true, .. }), "{status:?}");
        frame(&mut app, &ctx);
    }

    #[test]
    fn the_notice_buttons_do_what_they_say() {
        let ctx = egui::Context::default();
        let mut app = StrataApp::new(None, None);
        let u = app.update.updater().clone();
        // Not installable here: Install opens the release page instead.
        u.set_status(Status::Available { release: release(Err("Strata can't write to /Applications".into())), manual: false });
        frame(&mut app, &ctx);
        app.update_act(&ctx, Act::Install);
        assert!(matches!(u.status(), Status::Available { .. }), "nothing downloads");
        app.update_act(&ctx, Act::Later);
        assert_eq!(u.status(), Status::Idle);
        // Skip, with nowhere to save it (no config folder), still closes.
        u.set_status(Status::Available { release: release(Ok(())), manual: false });
        app.update_act(&ctx, Act::Skip);
        assert_eq!(u.status(), Status::Idle);
        assert!(app.status.contains("1.4.0"));
        // A message goes with OK.
        u.set_status(Status::Message("Strata was updated to 1.4.0.".into()));
        frame(&mut app, &ctx);
        app.update_act(&ctx, Act::Close);
        assert_eq!(u.status(), Status::Idle);
    }

    #[test]
    fn the_ask_card_sets_the_setting() {
        let ctx = egui::Context::default();
        let mut app = StrataApp::new(None, None);
        assert_eq!(app.settings.update_check, "ask");
        app.update_act(&ctx, Act::TurnOn);
        assert_eq!(app.settings.update_check, "auto");
        app.update_act(&ctx, Act::NotNow);
        assert_eq!(app.settings.update_check, "off");
    }

    #[test]
    fn notices_show_in_one_window() {
        let u = Updater::for_tests();
        assert!(u.notice_owner(1, true, 0.0, false), "the first window by default");
        assert!(!u.notice_owner(2, false, 0.1, false));
        assert!(u.notice_owner(2, false, 0.2, true), "a window that asked takes it");
        assert!(!u.notice_owner(1, true, 0.3, false));
        assert!(u.notice_owner(1, true, 2.0, false), "the asking window went away");
    }

    #[test]
    fn installed_quits_to_relaunch_and_notices_a_cancelled_quit() {
        let ctx = egui::Context::default();
        let mut app = StrataApp::new(None, None);
        let u = app.update.updater().clone();
        let plan = update::helper::Plan { pid: 1, target: "/x/Strata.app".into(), backup: "/x/.Strata.app.old".into(), version: "1.4.0".into() };
        u.set_status(Status::Installed { release: release(Ok(())), plan });
        app.update_relaunch(&ctx, 0.0);
        assert!(app.update.quitting.is_some());
        for i in 0..12 {
            app.update_relaunch(&ctx, 3.0 + i as f64);
        }
        assert!(app.quit_was_cancelled());
        app.update_act(&ctx, Act::Relaunch);
        app.update_relaunch(&ctx, 20.0);
        assert!(app.update.quitting.is_some_and(|(_, f)| f == 0), "asks to quit again");
    }

    /// The notices, for the manual: `STRATA_HEADLESS_SHOTS=dir cargo test --release update_headless_shots`.
    #[test]
    fn update_headless_shots() {
        let Some(dir) = std::env::var_os("STRATA_HEADLESS_SHOTS").map(std::path::PathBuf::from) else { return };
        let Some(mut shooter) = super::super::headless::Shooter::new(1100, 720, 2.0) else { return };
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        for (scheme, tag) in [("Strata Dark", "dark"), ("Strata Light", "light")] {
            let ctx = egui::Context::default();
            let mut app = StrataApp::new(Some(root.join("src/update/version.rs")), None);
            app.settings.theme = scheme.into();
            app.settings.minimap = false;
            app.apply_settings();
            shooter.run(&ctx, &mut app, 3, vec![]);
            let u = app.update.updater().clone();
            u.set_status(Status::Available { release: release(Ok(())), manual: true });
            shooter.shoot(&ctx, &mut app, 6, vec![], &dir.join(format!("update_available_{tag}.ppm")));
            u.set_status(Status::Downloading { release: release(Ok(())), done: 9_800_000, total: 24_100_000 });
            shooter.shoot(&ctx, &mut app, 3, vec![], &dir.join(format!("update_downloading_{tag}.ppm")));
            u.set_status(Status::Available { release: release(Err("Strata can't write to /Applications (Permission denied), where it's installed".into())), manual: true });
            shooter.shoot(&ctx, &mut app, 3, vec![], &dir.join(format!("update_browser_{tag}.ppm")));
            u.set_status(Status::Failed { error: "the download isn't signed by Strata's developer: the signature doesn't match".into(), release: Some(release(Ok(()))), manual: true });
            shooter.shoot(&ctx, &mut app, 3, vec![], &dir.join(format!("update_failed_{tag}.ppm")));
            u.set_status(Status::Idle);
            app.update.force_ask = true;
            shooter.shoot(&ctx, &mut app, 3, vec![], &dir.join(format!("update_ask_{tag}.ppm")));
        }
    }
}
