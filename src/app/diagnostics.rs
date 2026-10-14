//! Help ▸ Open Log Folder, Show Latest Log and Report a Problem…, and the
//! notice shown after a crash. Nothing here sends anything: the report is a
//! tab the user reads, edits and copies; a browser opens only on a click.

use super::StrataApp;
use crate::commands::Cmd;
use crate::crash;
#[cfg(test)]
use crate::crash::PreviousCrash;
use crate::document::Document;
use std::path::Path;

/// Log lines put in a problem report.
const REPORT_LOG_LINES: usize = 60;

impl StrataApp {
    pub(super) fn diagnostics_command(&mut self, cmd: Cmd) {
        let Some(config) = self.config.clone() else {
            self.status = "There's no settings folder, so nothing is logged".into();
            return;
        };
        let logs = crate::logging::log_dir(&config);
        match cmd {
            Cmd::OpenLogFolder => {
                let _ = std::fs::create_dir_all(&logs);
                // Reveal the log itself (selected), or the folder before there is one.
                let log = logs.join(crate::logging::FILE_NAME);
                let target = if log.exists() { log } else { logs };
                if let Err(e) = crate::ctxmenu::reveal_in_file_manager(&target) {
                    log::warn!("couldn't open the file manager at {}: {e}", target.display());
                    self.status = format!("Could not open the file manager: {e}");
                }
            }
            Cmd::ShowLatestLog => {
                let log = logs.join(crate::logging::FILE_NAME);
                if !log.exists() {
                    self.status = format!("No log yet ({} doesn't exist)", log.display());
                    return;
                }
                self.open_path(log.clone(), false);
                if let Some(d) = self.docs.get_mut(self.active).filter(|d| d.path.as_ref() == Some(&log)) {
                    d.editor.read_only = true;
                }
            }
            Cmd::ReportProblem => {
                let text = problem_report(Some(&config), &crate::logging::recent_lines());
                let mut doc = Document::from_buffer(None, crate::buffer::Buffer::from_str(&text));
                doc.title = Some("Report a Problem".into());
                doc.highlighter.set_syntax("Markdown");
                self.push_doc(doc);
                self.status = format!("Review the report, copy it, and paste it at {ISSUES}/new (nothing has been sent)", ISSUES = crash::ISSUES_URL);
            }
            _ => {}
        }
    }

    /// "Strata quit unexpectedly last time", in the corner until dismissed.
    pub(super) fn crash_notice_ui(&mut self, ctx: &egui::Context) {
        let Some(crash) = &self.crash_notice else { return };
        let mut action = None;
        egui::Window::new("Strata quit unexpectedly")
            .id(self.link.id("crash-notice"))
            .anchor(egui::Align2::RIGHT_BOTTOM, [-16.0, -40.0])
            .collapsible(false)
            .resizable(false)
            .title_bar(false)
            .show(ctx, |ui| {
                ui.set_max_width(380.0);
                ui.strong("Strata quit unexpectedly last time.");
                ui.label(crash.summary());
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    for (label, a) in [("Show Report", 0), ("Copy Report", 1), ("Report on GitHub…", 2), ("Dismiss", 3)] {
                        if ui.button(label).clicked() {
                            action = Some(a);
                        }
                    }
                });
            });
        let Some(action) = action else { return };
        let report = crash.report.clone();
        match action {
            0 => self.open_path(report, false),
            1 => match std::fs::read_to_string(&report) {
                Ok(text) => {
                    ctx.copy_text(text);
                    self.status = "Crash report copied".into();
                }
                Err(e) => self.status = format!("Could not read {}: {e}", report.display()),
            },
            2 => {
                if let Err(msg) = crate::links::open_web(ctx, &format!("{}/new", crash::ISSUES_URL)) {
                    self.status = msg;
                }
            }
            _ => self.crash_notice = None,
        }
    }

    #[cfg(test)]
    pub(super) fn set_crash_notice(&mut self, crash: PreviousCrash) {
        self.crash_notice = Some(crash);
    }
}

/// The text of Help ▸ Report a Problem…: a template to fill in, system
/// details, and the end of the log.
pub fn problem_report(config: Option<&Path>, log: &[String]) -> String {
    let mut s = String::new();
    s.push_str("<!-- Nothing has been sent. Fill this in, check it for anything private, then copy it into a new issue at\n");
    s.push_str(&format!("     {}/new — or attach it to an email. -->\n\n", crash::ISSUES_URL));
    s.push_str("## What happened\n\n\n\n## Steps to reproduce\n\n1.\n2.\n\n## What you expected\n\n\n\n## System\n\n");
    s.push_str(&format!("- {}\n- {}\n", crash::version_line(), crash::system_line()));
    if let Some(config) = config {
        s.push_str(&format!("- Settings folder: {}\n", config.display()));
        if let Some(report) = crash::latest_report(config) {
            s.push_str(&format!("- Latest crash report: {} (attach it if this is about a crash)\n", report.display()));
        }
    }
    let tail = &log[log.len().saturating_sub(REPORT_LOG_LINES)..];
    s.push_str("\n## Recent log\n\n```\n");
    for line in tail {
        s.push_str(line);
        s.push('\n');
    }
    s.push_str("```\n");
    crash::redact_home(&s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;
    use crate::editor::Editor;

    #[test]
    fn report_has_the_template_system_and_log_tail_without_the_home_folder() {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/Users/someone".into());
        let config = Path::new(&home).join("Library/Application Support/Strata-test-report");
        let log: Vec<String> = (0..100).map(|i| format!("2026-10-13T00:00:00.000Z INFO  [app] line {i} {home}/x.rs")).collect();
        let text = problem_report(Some(&config), &log);
        assert!(text.contains("## What happened") && text.contains("## Steps to reproduce"));
        assert!(text.contains(&crash::version_line()) && text.contains(&crash::system_line()));
        assert!(text.contains("line 99 ~/x.rs") && !text.contains("line 39 "), "the last lines only");
        assert!(text.contains("Settings folder: ~/Library/Application Support/Strata-test-report"));
        assert!(!text.contains(&format!("{home}/")), "home folder redacted");
        assert!(text.contains(crash::ISSUES_URL));
    }

    #[test]
    fn open_and_save_failures_log_the_path_but_never_the_text() {
        crate::logging::init_for_tests();
        let dir = std::env::temp_dir().join(format!("strata-diag-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a-folder")).unwrap();
        let mut app = StrataApp::new(None, None);
        app.docs[0].editor = Editor::new(Buffer::from_str("hunter2 private draft"));
        let nowhere = dir.join("missing-dir/notes.txt");
        app.docs[0].path = Some(nowhere.clone());
        app.docs[0].editor.insert_text("more private words");
        assert!(!app.save(0, false));
        app.open_path(dir.join("a-folder"), false);
        let lines: Vec<String> = crate::logging::recent_lines().into_iter().filter(|l| l.contains(&dir.display().to_string())).collect();
        assert!(lines.iter().any(|l| l.contains("couldn't save") && l.contains("missing-dir/notes.txt")), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("couldn't open") && l.contains("a-folder")), "{lines:?}");
        assert!(!crate::logging::recent_lines().iter().any(|l| l.contains("hunter2") || l.contains("private words")));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn report_a_problem_opens_a_tab_and_crash_notice_dismisses() {
        let dir = std::env::temp_dir().join(format!("strata-diag-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = StrataApp::new(None, Some(dir.clone()));
        app.docs[0].editor = Editor::new(Buffer::from_str("secret body text"));
        app.diagnostics_command(Cmd::ReportProblem);
        let d = &app.docs[app.active];
        assert_eq!(d.title.as_deref(), Some("Report a Problem"));
        assert!(d.editor.buffer.slice(0..d.editor.buffer.len_chars()).contains("## System"));
        assert!(!d.editor.buffer.slice(0..d.editor.buffer.len_chars()).contains("secret body text"));
        // The notice draws without a click doing anything.
        app.set_crash_notice(PreviousCrash { report: dir.join("Crashes/r.txt"), recovery: "session".into() });
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| app.crash_notice_ui(ui.ctx()));
        out.textures_delta.clear();
        assert!(app.crash_notice.is_some());
        drop(app);
        std::fs::remove_dir_all(dir).ok();
    }
}
