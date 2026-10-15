// No console window behind the editor on Windows release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
// `select_ranges(&[a..b])` is a one-selection list, not a mistaken range array.
#![allow(clippy::single_range_in_vec_init)]

mod app;
mod brackets;
mod build;
mod buffer;
mod cli;
mod commands;
mod crash;
mod compare;
mod ctxmenu;
mod decor;
mod dev;
mod document;
mod editor;
mod editorconfig;
mod encoding;
mod files;
mod find;
mod fold;
mod fonts;
mod fuzzy;
mod git;
mod help;
mod devshot;
mod diff;
mod highlight;
mod indent;
mod jsonc;
mod ipc;
mod keybindings;
mod keys;
mod menus;
mod lang;
mod links;
mod logging;
#[cfg(target_os = "macos")]
mod mac_open;
mod markdown;
mod palette;
mod panes;
mod pdf;
mod print;
mod private;
mod replace;
mod safe_html;
mod search;
mod session;
mod snippets;
mod spell;
mod shell_command;
mod sidebar;
mod terminal;
mod trash;
mod symbols;
mod view;
mod wrap;

use cli::{Cli, Target};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn main() -> ExitCode {
    let cwd = std::env::current_dir().unwrap_or_default();
    let (targets, wait, new_window) = match cli::parse(std::env::args_os().skip(1), &cwd) {
        Cli::Help => {
            print!("{}", cli::USAGE);
            return ExitCode::SUCCESS;
        }
        Cli::Version => {
            println!("strata {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Cli::Error(e) => {
            eprintln!("strata: {e}");
            return ExitCode::from(2);
        }
        Cli::Open { targets, wait, new_window } => (targets, wait, new_window),
    };
    let config = session::config_dir();
    // Already running: hand over the paths and get out of the way.
    if let Some(dir) = &config
        && let Some(code) = forward(dir, &targets, wait, new_window)
    {
        return code;
    }
    // Typed in a terminal (through the `strata` link into the app bundle): start
    // the app through Launch Services so it gets a Dock icon and focus, and return.
    #[cfg(target_os = "macos")]
    if let Some(bundle) = app_bundle()
        && std::io::IsTerminal::is_terminal(&std::io::stdin())
    {
        return launch_bundle(&bundle, config.as_deref(), &targets, wait, new_window);
    }
    run(targets, new_window, config)
}

/// Send the paths to a running Strata. `None` if there isn't one.
fn forward(dir: &Path, targets: &[Target], wait: bool, new_window: bool) -> Option<ExitCode> {
    match ipc::send(dir, targets, wait, new_window) {
        Ok(Some(sent)) if sent.ack.ok => {
            if wait {
                sent.wait_closed();
            }
            Some(ExitCode::SUCCESS)
        }
        Ok(_) => None,
        Err(e) => {
            eprintln!("strata: the running Strata didn't answer ({e}); opening another window");
            None
        }
    }
}

/// The `.app` this executable is inside, if any (following a `strata` symlink).
#[cfg(target_os = "macos")]
fn app_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().and_then(std::fs::canonicalize).ok()?;
    let bundle = exe.parent().filter(|p| p.ends_with("Contents/MacOS"))?.parent()?.parent()?;
    bundle.extension().is_some_and(|e| e == "app").then(|| bundle.to_path_buf())
}

#[cfg(target_os = "macos")]
fn launch_bundle(bundle: &Path, config: Option<&Path>, targets: &[Target], wait: bool, new_window: bool) -> ExitCode {
    let mut cmd = std::process::Command::new("open");
    cmd.arg("-a").arg(bundle);
    // Launch Services doesn't pass our environment on.
    if let Some(dir) = std::env::var_os("STRATA_CONFIG_DIR") {
        let mut kv = std::ffi::OsString::from("STRATA_CONFIG_DIR=");
        kv.push(dir);
        cmd.arg("--env").arg(kv);
    }
    if !wait && (!targets.is_empty() || new_window) {
        cmd.arg("--args");
        if new_window {
            cmd.arg("-n");
        }
        for t in targets {
            let mut a = t.path.clone().into_os_string();
            if let Some(line) = t.line {
                a.push(format!(":{line}"));
                if let Some(col) = t.col {
                    a.push(format!(":{col}"));
                }
            }
            cmd.arg(a);
        }
    }
    match cmd.status() {
        Ok(s) if s.success() => {}
        Ok(_) => return ExitCode::FAILURE,
        Err(e) => {
            eprintln!("strata: couldn't run open: {e}");
            return ExitCode::FAILURE;
        }
    }
    if !wait {
        return ExitCode::SUCCESS;
    }
    // --wait: once the app is up, hand it the files over the socket and block.
    let Some(dir) = config else { return ExitCode::SUCCESS };
    for _ in 0..300 {
        match ipc::send(dir, targets, true, new_window) {
            Ok(Some(sent)) => {
                sent.wait_closed();
                return ExitCode::SUCCESS;
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(50)),
            Err(e) => {
                eprintln!("strata: {e}");
                return ExitCode::FAILURE;
            }
        }
    }
    eprintln!("strata: Strata didn't start listening in time; not waiting");
    ExitCode::FAILURE
}

fn run(targets: Vec<Target>, new_window: bool, config: Option<PathBuf>) -> ExitCode {
    // Owner-only config folder (Unix): the session holds unsaved text. A
    // folder picked with STRATA_CONFIG_DIR may be shared, so only what Strata
    // keeps inside it is restricted.
    if let Some(dir) = &config {
        let _ = private::create_dir_all(dir);
        private::harden(dir, std::env::var_os("STRATA_CONFIG_DIR").is_none());
    }
    logging::init(config.as_deref());
    crash::install(config.as_deref());
    log::info!("{} starting on {}, settings in {}", crash::version_line(), crash::system_line(), config.as_deref().map_or("(none)".into(), |d| d.display().to_string()));
    #[cfg(target_os = "macos")]
    mac_open::install();
    // Every window is built the same way (see `app::windows::builder`).
    let options = eframe::NativeOptions { viewport: app::Windows::first_builder(config.as_deref()), ..Default::default() };
    let result = eframe::run_native(
        "Strata",
        options,
        Box::new(move |cc| {
            let mut windows = app::Windows::new(config.clone());
            if !targets.is_empty() || new_window {
                windows.open_targets(&targets, new_window);
            }
            #[cfg(target_os = "macos")]
            {
                windows.install_native_menu(&cc.egui_ctx);
                mac_open::set_context(&cc.egui_ctx);
            }
            // Only the process that owns the session answers other `strata`s,
            // and offers the report of a crash.
            let owner = config.filter(|_| windows.owns_session());
            if let Some(previous) = owner.as_deref().and_then(crash::check_previous) {
                log::warn!("Strata quit unexpectedly last time; report: {}", previous.report.display());
                windows.show_crash_notice(previous);
            }
            let server = owner.and_then(|dir| {
                let ctx = cc.egui_ctx.clone();
                ipc::Server::start(&dir, move || ctx.request_repaint()).inspect_err(|e| log::warn!("can't listen for other instances: {e}")).ok()
            });
            Ok(Box::new(Shell { windows, server, waiters: Vec::new(), dev_panic: crash::DevPanic::from_env(), rescued: false }))
        }),
    );
    match result {
        Ok(()) => {
            log::info!("quit");
            ExitCode::SUCCESS
        }
        Err(e) => {
            log::error!("couldn't start: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The editor windows plus what reaches them from outside: other `strata`
/// processes and (macOS) Finder.
struct Shell {
    windows: app::Windows,
    server: Option<ipc::Server>,
    /// `strata --wait` callers and the files they wait on.
    waiters: Vec<(Vec<PathBuf>, ipc::Waiter)>,
    /// STRATA_DEV_PANIC (dev aid; inert unless set).
    dev_panic: crash::DevPanic,
    /// Unsaved work was saved after a crash.
    rescued: bool,
}

impl Shell {
    /// Run one of eframe's calls into the app. A panic in it is a crash
    /// (the hook has written the report): save unsaved work and exit.
    fn guarded<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(self))) {
            Ok(r) => r,
            Err(_) => {
                self.rescue();
                // Remove the socket now: exit skips destructors.
                self.server = None;
                std::process::exit(101)
            }
        }
    }

    fn rescue(&mut self) {
        if !std::mem::replace(&mut self.rescued, true) {
            let windows = &mut self.windows;
            let rescue = crash::rescue(|| windows.emergency_save());
            crash::finish(&rescue);
        }
    }

    fn take_requests(&mut self, ctx: &egui::Context) {
        let requests = self.server.as_ref().map(ipc::Server::poll).unwrap_or_default();
        if !requests.is_empty() {
            let mut window = egui::ViewportId::ROOT;
            for req in requests {
                window = self.windows.open_targets(&req.targets, req.new_window);
                if let Some(w) = req.waiter {
                    self.waiters.push((req.targets.iter().filter(|t| !t.path.is_dir()).map(|t| t.path.clone()).collect(), w));
                }
            }
            ctx.send_viewport_cmd_to(window, egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd_to(window, egui::ViewportCommand::Focus);
            // macOS only lets an app come forward on its own when the user asked
            // for it; a request through Launch Services counts.
            #[cfg(target_os = "macos")]
            if let Some(bundle) = app_bundle()
                && let Ok(mut child) = std::process::Command::new("open").arg("-a").arg(bundle).spawn()
            {
                std::thread::spawn(move || child.wait());
            }
        }
        // Finder, the Dock and `open` activate the app themselves.
        #[cfg(target_os = "macos")]
        {
            let paths = mac_open::take();
            if !paths.is_empty() {
                self.windows.open_targets(&paths.into_iter().map(Target::plain).collect::<Vec<_>>(), false);
            }
        }
    }
}

/// A crash outside the calls `guarded` wraps (in eframe itself) unwinds
/// through here: save what can be saved.
impl Drop for Shell {
    fn drop(&mut self) {
        if std::thread::panicking() || crash::crashed() {
            self.rescue();
        }
    }
}

impl eframe::App for Shell {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.guarded(|s| {
            s.take_requests(ctx);
            s.windows.root().logic(ctx, frame);
        });
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.guarded(|s| {
            s.dev_panic.tick(ui.ctx());
            s.windows.ui(ui, frame);
            // Release `--wait` callers whose files are all closed.
            let windows = &s.windows;
            let (done, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut s.waiters).into_iter().partition(|(paths, _)| !paths.iter().any(|p| windows.has_open_file(p)));
            s.waiters = keep;
            done.into_iter().for_each(|(_, w)| w.done());
        });
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.guarded(|s| s.windows.root().save(storage));
    }

    fn on_exit(&mut self) {
        self.guarded(|s| s.windows.root().on_exit());
    }

    /// See-through windows need a transparent clear; the panels paint the color.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0; 4]
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        self.guarded(|s| s.windows.root().raw_input_hook(ctx, raw_input));
    }
}
