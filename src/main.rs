// No console window behind the editor on Windows release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
// `select_ranges(&[a..b])` is a one-selection list, not a mistaken range array.
#![allow(clippy::single_range_in_vec_init)]

mod app;
mod brackets;
mod build;
mod buffer;
mod commands;
mod ctxmenu;
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
mod indent;
mod keys;
mod menus;
mod lang;
mod markdown;
mod palette;
mod panes;
mod replace;
mod search;
mod session;
mod snippets;
mod sidebar;
mod terminal;
mod symbols;
mod view;
mod wrap;

use std::path::PathBuf;

fn main() -> eframe::Result {
    let path = std::env::args_os().nth(1).map(PathBuf::from);
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
    eframe::run_native(
        "Strata",
        options,
        Box::new(|_cc| {
            #[allow(unused_mut)]
            let mut app = app::StrataApp::new(path, session::config_dir());
            #[cfg(target_os = "macos")]
            app.install_native_menu(&_cc.egui_ctx);
            Ok(Box::new(app))
        }),
    )
}
