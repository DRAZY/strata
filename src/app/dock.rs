//! The macOS Dock icon's menu (see `mac_open`): each frame it gets the
//! current recent files and folders, and New Window chosen there opens one.

use super::StrataApp;

impl StrataApp {
    pub(super) fn dock_frame(&mut self, ctx: &egui::Context) {
        #[cfg(target_os = "macos")]
        {
            crate::mac_open::set_dock_recents(&self.recent_files, &self.recent_folders);
            if crate::mac_open::take_new_window() {
                self.run(ctx, crate::commands::Cmd::NewWindow);
            }
        }
        #[cfg(not(target_os = "macos"))]
        let _ = ctx;
    }
}
