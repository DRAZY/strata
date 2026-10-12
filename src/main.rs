// No console window behind the editor on Windows release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
// `select_ranges(&[a..b])` is a one-selection list, not a mistaken range array.
#![allow(clippy::single_range_in_vec_init)]

mod app;
mod buffer;
mod commands;
mod document;
mod editor;
mod files;
mod find;
mod fonts;
mod fuzzy;
mod devshot;
mod highlight;
mod keys;
mod lang;
mod palette;
mod panes;
mod search;
mod session;
mod sidebar;
mod symbols;
mod view;
mod wrap;

use std::path::PathBuf;

fn main() -> eframe::Result {
    let path = std::env::args_os().nth(1).map(PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 720.0])
            .with_title("Strata"),
        ..Default::default()
    };
    eframe::run_native(
        "Strata",
        options,
        Box::new(|cc| {
            fonts::install_symbol_fallback(&cc.egui_ctx);
            Ok(Box::new(app::StrataApp::new(path, session::config_dir())))
        }),
    )
}
