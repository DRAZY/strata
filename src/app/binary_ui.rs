//! Drawing tabs that aren't text (see `binary`): an image shrunk to fit
//! the pane (click for 100%, click again to fit), or a hex dump under a
//! "Binary file — read-only" banner, laid out a screenful of rows at a time.

use crate::binary::{BinaryView, Content, ROW};
use crate::document::Document;
use crate::highlight::Highlighter;
use egui::{Color32, CursorIcon, FontId, Rect, Response, RichText, Sense, Stroke, Ui, pos2, vec2};

/// The status bar's description of a binary tab, instead of the cursor position.
pub(super) fn status(d: &Document) -> Option<String> {
    d.binary.as_ref().map(|b| b.describe())
}

/// Draw `view` filling `ui`; the response is for its right-click menu.
pub(super) fn show(view: &mut BinaryView, ui: &mut Ui, hl: &Highlighter, font_size: f32, id: egui::Id) -> Response {
    let rect = ui.max_rect();
    ui.painter().rect_filled(rect, 0.0, hl.background());
    let resp = ui.interact(rect, id, Sense::click());
    let len = view.len;
    match &mut view.content {
        Content::Image { picture, texture, actual_size, scale } => {
            let texture = texture.get_or_insert_with(|| ui.ctx().load_texture(format!("strata-image-{id:?}"), std::mem::take(&mut picture.pixels), egui::TextureOptions::LINEAR));
            let ppp = ui.ctx().pixels_per_point();
            // At 100% an image pixel is a screen pixel.
            let full = vec2(picture.width as f32, picture.height as f32) / ppp;
            let room = rect.shrink(16.0).size();
            let fit = (room.x / full.x).min(room.y / full.y).min(1.0);
            *scale = if *actual_size { 1.0 } else { fit };
            let size = full * *scale;
            let mut clicked = false;
            let mut scroll = |ui: &mut Ui| {
                let (place, r) = ui.allocate_exact_size(size.max(room), Sense::click());
                let at = Rect::from_center_size(place.center(), size);
                checkerboard(ui, at.intersect(ui.clip_rect()), hl);
                ui.painter().image(texture.id(), at, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                ui.painter().rect_stroke(at, 0.0, Stroke::new(1.0, hl.foreground().gamma_multiply(0.15)), egui::StrokeKind::Outside);
                // Clicking toggles 100% when that differs from fitting.
                if fit < 1.0 {
                    clicked = r.clicked();
                    r.on_hover_cursor(if *actual_size { CursorIcon::ZoomOut } else { CursorIcon::ZoomIn });
                }
            };
            ui.scope_builder(egui::UiBuilder::new().max_rect(rect.shrink(8.0)), |ui| {
                if *actual_size {
                    egui::ScrollArea::both().id_salt(id.with("image")).auto_shrink(false).show(ui, |ui| scroll(ui));
                } else {
                    scroll(ui);
                }
            });
            if clicked {
                *actual_size = !*actual_size;
            }
        }
        Content::Hex(reader) => {
            let font = FontId::monospace(font_size);
            let row_h = ui.fonts_mut(|f| f.row_height(&font)) + 2.0;
            let digits = crate::binary::offset_digits(len);
            let rows = len.div_ceil(ROW as u64) as usize;
            let (fg, weak, accent) = (hl.foreground(), hl.foreground().gamma_multiply(0.5), hl.accent());
            ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                banner(ui, len, hl);
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                ui.spacing_mut().item_spacing.y = 0.0;
                egui::ScrollArea::both().id_salt(id.with("hex")).auto_shrink(false).show_rows(ui, row_h, rows, |ui, range| {
                    let first = range.start as u64 * ROW as u64;
                    let bytes = reader.read(first, range.len() * ROW).to_vec();
                    for (k, chunk) in bytes.chunks(ROW).enumerate() {
                        let line = crate::binary::hex_row(first + (k * ROW) as u64, chunk, digits);
                        // Offset, hex, ASCII in three tones.
                        let (offset, rest) = line.split_at(digits);
                        let (hex, ascii) = rest.split_at(2 + ROW * 3 + 1);
                        let mut job = egui::text::LayoutJob::default();
                        for (text, color) in [(offset, weak), (hex, fg), (ascii, accent)] {
                            job.append(text, 0.0, egui::TextFormat { font_id: font.clone(), color, ..Default::default() });
                        }
                        // Painted, not laid out as a label: every row exactly `row_h` high.
                        let galley = ui.fonts_mut(|f| f.layout_job(job));
                        let (r, _) = ui.allocate_exact_size(vec2(galley.size().x + 24.0, row_h), Sense::hover());
                        ui.painter().galley(r.min + vec2(12.0, 1.0), galley, fg);
                    }
                });
            });
        }
    }
    resp
}

/// The strip above the hex view.
fn banner(ui: &mut Ui, len: u64, hl: &Highlighter) {
    let warn = Color32::from_rgb(0xe5, 0x9e, 0x40);
    egui::Frame::NONE.fill(warn.gamma_multiply(0.15)).inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(RichText::new("Binary file — read-only").strong().color(warn));
            ui.label(RichText::new(format!("· {} · not text, so it's shown as bytes and can't be edited or saved", crate::binary::size_label(len))).color(hl.foreground().gamma_multiply(0.7)));
        });
    });
}

/// Squares behind a picture, so transparent parts show as such.
fn checkerboard(ui: &Ui, area: Rect, hl: &Highlighter) {
    if !area.is_positive() {
        return;
    }
    const CELL: f32 = 10.0;
    let p = ui.painter();
    let base = hl.background();
    let light = if base.r() as u32 + base.g() as u32 + base.b() as u32 > 384 { Color32::from_gray(255) } else { Color32::from_gray(70) };
    let dark = if light == Color32::from_gray(255) { Color32::from_gray(225) } else { Color32::from_gray(50) };
    p.rect_filled(area, 0.0, light);
    let (cols, rows) = ((area.width() / CELL).ceil() as usize, (area.height() / CELL).ceil() as usize);
    for r in 0..rows {
        for c in (r % 2..cols).step_by(2) {
            let min = area.min + vec2(c as f32 * CELL, r as f32 * CELL);
            p.rect_filled(Rect::from_min_size(min, vec2(CELL, CELL)).intersect(area), 0.0, dark);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::StrataApp;
    use egui::{Context, Pos2, RawInput};

    fn frame(app: &mut StrataApp, ctx: &Context) {
        let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(900.0, 500.0))), ..Default::default() };
        let mut out = ctx.run_ui(raw, |ui| eframe::App::ui(app, ui, &mut eframe::Frame::_new_kittest()));
        out.textures_delta.clear();
    }

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("strata-binary-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn binary_files_open_read_only_in_the_hex_view_and_never_save() {
        let dir = scratch("hex");
        let path = dir.join("prog.bin");
        let bytes: Vec<u8> = b"\x7fELF\x02\x01\x01\0".iter().copied().chain((0..=255u8).cycle().take(5000)).collect();
        std::fs::write(&path, &bytes).unwrap();
        let ctx = Context::default();
        let mut app = StrataApp::new(Some(path.clone()), None);
        let d = &app.docs[app.active];
        assert!(matches!(d.binary.as_deref().map(|b| &b.content), Some(Content::Hex(_))));
        assert!(d.editor.read_only && !d.is_dirty());
        assert_eq!(status(d).as_deref(), Some("Binary · 4.9 KB (5,008 bytes)"));
        for _ in 0..3 {
            frame(&mut app, &ctx);
        }
        // Nothing saves it: Save, Save As and Save with Encoding all stop.
        assert!(!app.save(app.active, false));
        assert!(app.status.contains("can't be saved"), "{}", app.status);
        app.save_with_encoding(crate::encoding::Encoding::UTF16LE);
        assert!(!app.docs[app.active].is_dirty());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);

        // Changed on disk: the view follows.
        std::fs::write(&path, &bytes[..100]).unwrap();
        app.docs[app.active].disk_mtime = Some(std::time::SystemTime::UNIX_EPOCH);
        assert!(app.docs[app.active].check_disk());
        assert_eq!(app.docs[app.active].binary.as_ref().unwrap().len, 100);

        // Reopen with Encoding says it's text after all.
        app.reopen_with_encoding(crate::encoding::Encoding::WINDOWS_1252);
        let d = &app.docs[app.active];
        assert!(d.binary.is_none() && !d.editor.read_only);
        assert!(d.editor.buffer.line_text(0).starts_with("\u{7f}ELF"));
        frame(&mut app, &ctx);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn images_open_in_a_preview() {
        let dir = scratch("img");
        let path = dir.join("dot.png");
        let img = image::RgbaImage::from_fn(64, 32, |x, y| image::Rgba([x as u8 * 4, y as u8 * 8, 128, if x < 8 { 0 } else { 255 }]));
        img.save(&path).unwrap();
        let ctx = Context::default();
        let mut app = StrataApp::new(Some(path.clone()), None);
        let d = &app.docs[app.active];
        assert!(matches!(d.binary.as_deref().map(|b| &b.content), Some(Content::Image { .. })));
        assert!(status(d).unwrap().starts_with("PNG image · 64 × 32 · "), "{:?}", status(d));
        for _ in 0..3 {
            frame(&mut app, &ctx);
        }
        let Some(Content::Image { texture, scale, .. }) = app.docs[app.active].binary.as_deref().map(|b| &b.content) else { panic!() };
        assert!(texture.is_some(), "uploaded on the first draw");
        assert_eq!(*scale, 1.0, "small images show at 100%");
        assert!(status(&app.docs[app.active]).unwrap().ends_with("· 100%"));

        // A broken image shows as bytes; text named .png is still text.
        std::fs::write(dir.join("broken.png"), b"\x89PNG\r\n\x1a\n\0\0\0\rnot really").unwrap();
        app.open_path(dir.join("broken.png"), false);
        assert!(matches!(app.docs[app.active].binary.as_deref().map(|b| &b.content), Some(Content::Hex(_))));
        std::fs::write(dir.join("notes.png"), "just text\n").unwrap();
        app.open_path(dir.join("notes.png"), false);
        assert!(app.docs[app.active].binary.is_none());
        std::fs::remove_dir_all(dir).ok();
    }
}
