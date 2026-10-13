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
mod decor;
mod document;
mod editor;
mod encoding;
mod files;
mod find;
mod fold;
mod fonts;
mod fuzzy;
mod git;
mod devshot;
mod highlight;
mod ipc;
mod keys;
mod menus;
mod lang;
#[cfg(target_os = "macos")]
mod mac_open;
mod markdown;
mod palette;
mod panes;
mod search;
mod session;
mod shell_command;
mod sidebar;
mod terminal;
mod symbols;
mod view;
mod wrap;

use cli::{Cli, Target};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn main() -> ExitCode {
    let cwd = std::env::current_dir().unwrap_or_default();
    let (targets, wait) = match cli::parse(std::env::args_os().skip(1), &cwd) {
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
        Cli::Open { targets, wait } => (targets, wait),
    };
    let config = session::config_dir();
    // Already running: hand over the paths and get out of the way.
    if let Some(dir) = &config
        && let Some(code) = forward(dir, &targets, wait)
    {
        return code;
    }
    // Typed in a terminal (through the `strata` link into the app bundle): start
    // the app through Launch Services so it gets a Dock icon and focus, and return.
    #[cfg(target_os = "macos")]
    if let Some(bundle) = app_bundle()
        && std::io::IsTerminal::is_terminal(&std::io::stdin())
    {
        return launch_bundle(&bundle, config.as_deref(), &targets, wait);
    }
    run(targets, config)
}

/// Send the paths to a running Strata. `None` if there isn't one.
fn forward(dir: &Path, targets: &[Target], wait: bool) -> Option<ExitCode> {
    match ipc::send(dir, targets, wait) {
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
fn launch_bundle(bundle: &Path, config: Option<&Path>, targets: &[Target], wait: bool) -> ExitCode {
    let mut cmd = std::process::Command::new("open");
    cmd.arg("-a").arg(bundle);
    // Launch Services doesn't pass our environment on.
    if let Some(dir) = std::env::var_os("STRATA_CONFIG_DIR") {
        let mut kv = std::ffi::OsString::from("STRATA_CONFIG_DIR=");
        kv.push(dir);
        cmd.arg("--env").arg(kv);
    }
    if !wait && !targets.is_empty() {
        cmd.arg("--args");
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
        match ipc::send(dir, targets, true) {
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

fn run(targets: Vec<Target>, config: Option<PathBuf>) -> ExitCode {
    #[cfg(target_os = "macos")]
    mac_open::install();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 720.0])
            .with_title("Strata")
            // Lets View ▸ Window Opacity see through to the desktop.
            .with_transparent(true)
            // macOS: the window is see-through, so Strata paints its own solid title bar
            // under the traffic lights (see `title_bar`) instead of the system one.
            .with_fullsize_content_view(true)
            .with_titlebar_shown(false)
            .with_title_shown(false)
            // Otherwise eframe shows its own logo in the Dock and taskbar.
            .with_icon(eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon-app.png")).expect("bundled icon")),
        ..Default::default()
    };
    let result = eframe::run_native(
        "Strata",
        options,
        Box::new(move |cc| {
            #[allow(unused_mut)]
            let mut app = app::StrataApp::new(None, config.clone());
            app.open_targets(&targets);
            #[cfg(target_os = "macos")]
            {
                app.install_native_menu(&cc.egui_ctx);
                mac_open::set_context(&cc.egui_ctx);
            }
            // Only the window that owns the session answers other `strata`s.
            let server = config.filter(|_| app.owns_session()).and_then(|dir| {
                let ctx = cc.egui_ctx.clone();
                ipc::Server::start(&dir, move || ctx.request_repaint())
                    .inspect_err(|e| eprintln!("strata: can't listen for other instances: {e}"))
                    .ok()
            });
            Ok(Box::new(Shell { app, server, waiters: Vec::new() }))
        }),
    );
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("strata: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The editor plus what reaches it from outside: other `strata` processes and
/// (macOS) Finder. Every `eframe::App` method is passed through to the editor.
struct Shell {
    app: app::StrataApp,
    server: Option<ipc::Server>,
    /// `strata --wait` callers and the files they wait on.
    waiters: Vec<(Vec<PathBuf>, ipc::Waiter)>,
}

impl Shell {
    fn take_requests(&mut self, ctx: &egui::Context) {
        let requests = self.server.as_ref().map(ipc::Server::poll).unwrap_or_default();
        if !requests.is_empty() {
            for req in requests {
                self.app.open_targets(&req.targets);
                if let Some(w) = req.waiter {
                    self.waiters.push((req.targets.iter().filter(|t| !t.path.is_dir()).map(|t| t.path.clone()).collect(), w));
                }
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
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
                self.app.open_targets(&paths.into_iter().map(Target::plain).collect::<Vec<_>>());
            }
        }
    }
}

impl eframe::App for Shell {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.take_requests(ctx);
        self.app.logic(ctx, frame);
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.app.ui(ui, frame);
        // Release `--wait` callers whose files are all closed.
        let app = &self.app;
        let (done, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut self.waiters).into_iter().partition(|(paths, _)| !paths.iter().any(|p| app.has_open_file(p)));
        self.waiters = keep;
        done.into_iter().for_each(|(_, w)| w.done());
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.app.save(storage);
    }

    fn on_exit(&mut self) {
        self.app.on_exit();
    }

    fn auto_save_interval(&self) -> std::time::Duration {
        self.app.auto_save_interval()
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        self.app.clear_color(visuals)
    }

    fn persist_egui_memory(&self) -> bool {
        self.app.persist_egui_memory()
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        self.app.raw_input_hook(ctx, raw_input);
    }
}
