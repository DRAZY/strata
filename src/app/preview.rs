//! Window glue for the Markdown preview panel and HTML export (see `crate::markdown`).

use super::StrataApp;
use crate::markdown::Action;

impl StrataApp {
    /// The live preview of the active document, docked on the right.
    pub(super) fn markdown_panel(&mut self, ui: &mut egui::Ui) {
        if !self.settings.markdown_preview || self.distraction.is_some() {
            return;
        }
        let d = &mut self.docs[self.active];
        let bg = d.highlighter.background().gamma_multiply(self.settings.window_opacity.clamp(0.3, 1.0));
        let width = ui.available_width();
        self.markdown.font_size = self.settings.font_size;
        let out = egui::Panel::right(self.link.id("markdown-preview"))
            .resizable(true)
            .default_size(width * 0.45)
            .min_size(180.0)
            .frame(egui::Frame::new().fill(bg))
            .show(ui, |ui| self.markdown.show(ui, d));
        self.preview_context_menu(&out.response);
        let action = out.inner;
        match action {
            Some(Action::Open(p)) => self.open_path(p, false),
            Some(Action::Status(s)) => self.status = s,
            None => {}
        }
    }

    pub(super) fn export_markdown_html(&mut self) {
        let d = &mut self.docs[self.active];
        let stem = d.path.as_ref().and_then(|p| p.file_stem()).map_or_else(|| "Untitled".to_owned(), |s| s.to_string_lossy().into_owned());
        let mut dialog = rfd::FileDialog::new().set_file_name(format!("{stem}.html")).add_filter("HTML", &["html", "htm"]);
        if let Some(dir) = d.path.as_ref().and_then(|p| p.parent()) {
            dialog = dialog.set_directory(dir);
        }
        let Some(out) = dialog.save_file() else { return };
        let html = crate::markdown::to_html_document(&d.editor.text_snapshot(), &stem);
        self.status = match std::fs::write(&out, html) {
            Ok(()) => format!("Exported {}", out.display()),
            Err(e) => format!("Couldn't export: {e}"),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::Cmd;
    use egui::{Context, Pos2, RawInput, Rect, vec2};

    fn frame(app: &mut StrataApp, ctx: &Context) {
        let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 700.0))), ..Default::default() };
        let mut out = ctx.run_ui(raw, |ui| {
            let mut frame = eframe::Frame::_new_kittest();
            eframe::App::ui(app, ui, &mut frame);
        });
        out.textures_delta.clear();
    }

    #[test]
    fn preview_panel_toggles_follows_edits_and_hides_when_distraction_free() {
        let dir = std::env::temp_dir().join(format!("strata-preview-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("readme.md");
        std::fs::write(&file, "# Hi\n\n- [x] done\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n```rust\nfn x() {}\n```\n").unwrap();
        let ctx = Context::default();
        let mut app = StrataApp::new(Some(file), None);
        assert!(!app.settings.markdown_preview, "off by default");
        frame(&mut app, &ctx);
        assert!(app.markdown.blocks().is_empty(), "hidden panels parse nothing");
        app.run(&ctx, Cmd::ToggleMarkdownPreview);
        assert!(app.settings.markdown_preview);
        for _ in 0..3 {
            frame(&mut app, &ctx);
        }
        assert_eq!(app.markdown.blocks().len(), 4);
        app.doc().editor.insert_text("Intro\n\n");
        frame(&mut app, &ctx);
        assert_eq!(app.markdown.blocks().len(), 5, "live as you type");

        // Distraction-free hides the panel: edits there aren't previewed.
        app.run(&ctx, Cmd::DistractionFree);
        app.doc().editor.insert_text("More\n\n");
        frame(&mut app, &ctx);
        assert_eq!(app.markdown.blocks().len(), 5);
        app.run(&ctx, Cmd::DistractionFree);
        frame(&mut app, &ctx);
        assert_eq!(app.markdown.blocks().len(), 6);

        // A plain-text tab gets a hint; the toggle still works.
        app.push_doc(crate::document::Document::untitled());
        frame(&mut app, &ctx);
        app.run(&ctx, Cmd::ToggleMarkdownPreview);
        frame(&mut app, &ctx);
        assert!(!app.settings.markdown_preview);
        std::fs::remove_dir_all(dir).ok();
    }
}
