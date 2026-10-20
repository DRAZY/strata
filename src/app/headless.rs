//! Test aid: render whole app frames off screen with wgpu (no window, so it
//! works with the screen locked or in CI with a GPU) and save them as PPM.
//! `STRATA_HEADLESS_SHOTS=<folder> cargo test headless_shots` writes the
//! scenes below; convert with `sips -s format png`.

use super::StrataApp;
use eframe::egui_wgpu::{self, wgpu};
use egui::{Context, Event, Pos2, RawInput, Rect, vec2};
use std::path::Path;

/// Run a future to completion on this thread (wgpu's native futures are ready
/// once the device is polled, so spinning is fine here).
fn block_on<F: std::future::Future>(f: F) -> F::Output {
    let waker = std::task::Waker::noop();
    let mut cx = std::task::Context::from_waker(waker);
    let mut f = std::pin::pin!(f);
    loop {
        if let std::task::Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::yield_now();
    }
}

pub(super) struct Shooter {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: egui_wgpu::Renderer,
    size: [u32; 2],
    ppp: f32,
    /// Seconds of input time so far (animations need it to move).
    clock: f64,
}

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

impl Shooter {
    pub fn new(width: u32, height: u32, ppp: f32) -> Option<Self> {
        let instance = wgpu::Instance::default();
        let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
        let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()?;
        let renderer = egui_wgpu::Renderer::new(&device, FORMAT, egui_wgpu::RendererOptions::default());
        Some(Self { device, queue, renderer, size: [(width as f32 * ppp) as u32, (height as f32 * ppp) as u32], ppp, clock: 0.0 })
    }

    /// Run `frames` frames of `app` (input `events` on the first), then
    /// paint the last one into `path` (PPM).
    pub fn shoot(&mut self, ctx: &Context, app: &mut StrataApp, frames: usize, events: Vec<Event>, path: &Path) {
        let Some(out) = self.run(ctx, app, frames, events) else { return };
        self.paint(ctx, out, path);
    }

    /// Run frames without painting them (let the layout settle first).
    pub fn run(&mut self, ctx: &Context, app: &mut StrataApp, frames: usize, mut events: Vec<Event>) -> Option<egui::FullOutput> {
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(self.size[0] as f32 / self.ppp, self.size[1] as f32 / self.ppp));
        let mut last = None;
        // Frames run elsewhere on this context may have moved its clock on;
        // never go back in time, or animations (popups fading in) stall.
        self.clock = self.clock.max(ctx.input(|i| i.time) + 0.25);
        for f in 0..frames {
            let time = Some(self.clock + f as f64 * 0.25);
            let mut raw = RawInput { screen_rect: Some(screen), events: std::mem::take(&mut events), time, ..Default::default() };
            raw.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(self.ppp);
            ctx.set_pixels_per_point(self.ppp);
            let mut out = ctx.run_ui(raw, |ui| {
                let mut frame = eframe::Frame::_new_kittest();
                eframe::App::ui(app, ui, &mut frame);
            });
            for (id, deltas) in &out.textures_delta.set {
                for delta in deltas {
                    self.renderer.update_texture(&self.device, &self.queue, *id, delta);
                }
            }
            for id in &out.textures_delta.free {
                self.renderer.free_texture(id);
            }
            // Applied above (epaint asserts unhandled deltas in debug builds).
            out.textures_delta.clear();
            last = Some(out);
        }
        self.clock += frames as f64 * 0.25;
        last
    }

    fn paint(&mut self, ctx: &Context, out: egui::FullOutput, path: &Path) {
        let jobs = ctx.tessellate(out.shapes, self.ppp);
        let screen_desc = egui_wgpu::ScreenDescriptor { size_in_pixels: self.size, pixels_per_point: self.ppp };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shot"),
            size: wgpu::Extent3d { width: self.size[0], height: self.size[1], depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("shot") });
        let extra = self.renderer.update_buffers(&self.device, &self.queue, &mut encoder, &jobs, &screen_desc);
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shot"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.renderer.render(&mut pass.forget_lifetime(), &jobs, &screen_desc);
        }
        let row = (self.size[0] * 4).div_ceil(256) * 256;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor { label: Some("shot"), size: (row * self.size[1]) as u64, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo { buffer: &buffer, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: None } },
            texture.size(),
        );
        self.queue.submit(extra.into_iter().chain(std::iter::once(encoder.finish())));
        buffer.slice(..).map_async(wgpu::MapMode::Read, |r| r.expect("map"));
        self.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).expect("poll");
        let data = buffer.slice(..).get_mapped_range().expect("mapped").to_vec();
        let mut ppm = format!("P6\n{} {}\n255\n", self.size[0], self.size[1]).into_bytes();
        for y in 0..self.size[1] as usize {
            let line = &data[y * row as usize..][..self.size[0] as usize * 4];
            for px in line.chunks(4) {
                ppm.extend_from_slice(&px[..3]);
            }
        }
        std::fs::write(path, ppm).expect("write shot");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Scenes for the manual: a split view, a Compare tab, spell check, About.
    #[test]
    fn headless_shots() {
        let Some(dir) = std::env::var_os("STRATA_HEADLESS_SHOTS").map(PathBuf::from) else { return };
        let Some(mut shooter) = Shooter::new(1400, 820, 2.0) else { return };
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

        // Split View: one buffer in two panes, scrolled apart; an edit in the
        // right view shows up in the left one.
        let ctx = Context::default();
        let mut app = StrataApp::new(Some(root.join("src/diff.rs")), None);
        app.settings.minimap = false;
        shooter.run(&ctx, &mut app, 3, vec![]);
        app.split_view(0);
        app.goto_line(255);
        app.docs[1].editor.insert_text("// edited in the right view\n");
        app.activate(0);
        app.goto_line(238);
        shooter.shoot(&ctx, &mut app, 6, vec![], &dir.join("split.ppm"));

        // Compare: an older version of a file against the current one.
        let old = std::fs::read_to_string(root.join("src/diff.rs")).unwrap();
        let old = old.replacen("Lines longer than this (chars) are marked changed as a whole.", "Long lines are marked whole.", 1).replacen("    let mut budget = GAP_BUDGET / 16;\n", "", 1).replacen("fn push_merged", "fn push_merged_ranges", 1);
        let tmp = std::env::temp_dir().join("strata-shot-diff-old.rs");
        std::fs::write(&tmp, old).unwrap();
        let ctx = Context::default();
        let mut app = StrataApp::new(Some(root.join("src/diff.rs")), None);
        shooter.run(&ctx, &mut app, 3, vec![]);
        app.compare_with_file(0, &tmp);
        app.step_difference(true);
        app.step_difference(true);
        shooter.shoot(&ctx, &mut app, 6, vec![], &dir.join("compare.ppm"));

        // Compare with Japanese text on both sides (the CJK fonts load for it).
        let (old, new) = (std::env::temp_dir().join("strata-shot-cjk-old.txt"), std::env::temp_dir().join("strata-shot-cjk-new.txt"));
        std::fs::write(&old, "# メモ\n\n日本語のテキストファイルです。\nこれは文字コードの判定のためのテストです。\n同じ行です。\n").unwrap();
        std::fs::write(&new, "# メモ\n\n日本語の文章ファイルです。\nこれは文字コードの判定のためのテストです。\n同じ行です。\n新しい行を追加しました。\n").unwrap();
        let ctx = Context::default();
        let mut app = StrataApp::new(Some(new), None);
        shooter.run(&ctx, &mut app, 3, vec![]);
        app.compare_with_file(0, &old);
        app.step_difference(true);
        shooter.shoot(&ctx, &mut app, 6, vec![], &dir.join("compare_cjk.ppm"));

        // Trojan Source: direction-changing and invisible characters show as boxes,
        // the status bar counts them, and Compare shows them too.
        let trojan = "// Shown plainly, the check below looks commented out.\nfunction run(isAdmin) {\n    /*\u{202E} } \u{2066}if (isAdmin)\u{2069} \u{2066} begin admins only */\n        console.log(\"You are an admin.\");\n    /* end admins only \u{202E} { \u{2066}*/\n    let user\u{200B}Name = \"ad\u{00AD}min\"; // zero-width space, soft hyphen\n    const greeting = \"\u{2067}olleh\u{2069}\";\n}\n";
        let tmp = std::env::temp_dir().join("strata-shot-trojan.js");
        std::fs::write(&tmp, trojan).unwrap();
        let ctx = Context::default();
        let mut app = StrataApp::new(Some(tmp.clone()), None);
        app.settings.minimap = false;
        shooter.run(&ctx, &mut app, 3, vec![]);
        app.find_hidden_characters();
        shooter.shoot(&ctx, &mut app, 6, vec![], &dir.join("hidden.ppm"));
        let plain = std::env::temp_dir().join("strata-shot-trojan-plain.js");
        std::fs::write(&plain, trojan.chars().filter(|&c| !crate::hidden::is_hidden(c)).collect::<String>()).unwrap();
        app.compare_with_file(0, &plain);
        shooter.shoot(&ctx, &mut app, 6, vec![], &dir.join("hidden_compare.ppm"));

        // Spell check: prose and comments are checked, code isn't.
        let tmp = std::env::temp_dir().join("strata-shot-spell.md");
        std::fs::write(&tmp, "# Notes\n\nThis paragraph has a mispelled word and anothr one.\n\n```rust\n// a coment in code\nlet varible = 1;\n```\n").unwrap();
        let ctx = Context::default();
        let mut app = StrataApp::new(Some(tmp), None);
        app.settings.spell_check = true;
        for _ in 0..20 {
            shooter.run(&ctx, &mut app, 1, vec![]);
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        shooter.shoot(&ctx, &mut app, 4, vec![], &dir.join("spell.ppm"));

        // A Shift_JIS file: opened with a guessed encoding, which the status bar points out.
        let tmp = std::env::temp_dir().join("strata-shot-sjis.txt");
        let text = "日本語のテキストファイルです。\nこれは文字コードの判定のためのテストです。\n".repeat(4);
        std::fs::write(&tmp, encoding_rs::SHIFT_JIS.encode(&text).0).unwrap();
        let ctx = Context::default();
        let mut app = StrataApp::new(Some(tmp), None);
        shooter.shoot(&ctx, &mut app, 4, vec![], &dir.join("encoding_hint.ppm"));

        // An image preview (shrunk to fit; the left part transparent) and the hex view.
        let tmp = std::env::temp_dir().join("strata-shot-image.png");
        let img = image::RgbaImage::from_fn(3200, 2000, |x, y| {
            let (fx, fy) = (x as f32 / 3200.0, y as f32 / 2000.0);
            let ring = (((fx - 0.6).powi(2) + (fy - 0.5).powi(2)).sqrt() * 40.0).sin() * 0.5 + 0.5;
            image::Rgba([(255.0 * fx) as u8, (200.0 * ring) as u8, (255.0 * (1.0 - fy)) as u8, if x < 600 { (255.0 * x as f32 / 600.0) as u8 } else { 255 }])
        });
        img.save(&tmp).unwrap();
        let ctx = Context::default();
        let mut app = StrataApp::new(Some(tmp), None);
        shooter.shoot(&ctx, &mut app, 4, vec![], &dir.join("image_preview.ppm"));
        let tmp = std::env::temp_dir().join("strata-shot-hex.bin");
        let mut bytes = b"\x7fELF\x02\x01\x01\0\0\0\0\0\0\0\0\0\x02\0>\0\x01\0\0\0".to_vec();
        bytes.extend((0..4000u32).map(|i| if i % 7 == 0 { b"Strata hex view "[(i / 7 % 16) as usize] } else { (i * 37 % 256) as u8 }));
        std::fs::write(&tmp, bytes).unwrap();
        let ctx = Context::default();
        let mut app = StrataApp::new(Some(tmp), None);
        shooter.shoot(&ctx, &mut app, 4, vec![], &dir.join("hex_view.ppm"));

        // The About window over an empty editor.
        let ctx = Context::default();
        let mut app = StrataApp::new(None, None);
        app.about_open = true;
        shooter.shoot(&ctx, &mut app, 4, vec![], &dir.join("about.ppm"));

        // The Settings window (docked: no native windows here), dark and light,
        // a category and a search; a changed setting shows its Reset.
        for (scheme, tag) in [("Strata Dark", "dark"), ("Strata Light", "light")] {
            let ctx = Context::default();
            let mut app = StrataApp::new(Some(root.join("src/diff.rs")), None);
            app.settings.theme = scheme.into();
            app.settings.font_size = 15.0;
            app.settings.rulers = vec![80, 100];
            app.apply_settings();
            // Frames first: the first ones lay out at the wrong scale.
            shooter.run(&ctx, &mut app, 3, vec![]);
            app.open_settings_window();
            shooter.shoot(&ctx, &mut app, 6, vec![], &dir.join(format!("settings_{tag}.ppm")));
            app.settings_ui.search = "save".into();
            shooter.shoot(&ctx, &mut app, 4, vec![], &dir.join(format!("settings_search_{tag}.ppm")));
            app.settings_ui.search = "wrap".into();
            shooter.shoot(&ctx, &mut app, 4, vec![], &dir.join(format!("settings_wrap_{tag}.ppm")));
            app.settings_ui.search.clear();
            app.settings.added_words = vec!["Strata".into(), "egui".into()];
            app.settings_ui.category_for_shots(crate::settings_meta::Category::Find);
            shooter.shoot(&ctx, &mut app, 4, vec![], &dir.join(format!("settings_find_{tag}.ppm")));
        }

        // Review Changes: an agent's edits in a sample repository, the
        // Changes list and a file's changes with Keep and Revert, dark and light.
        if let Some(repo) = crate::review::tests::temp_repo("shots") {
            let main = "use std::io;\n\nfn main() {\n    let name = read_name();\n    println!(\"Hello, {name}!\");\n}\n\nfn read_name() -> String {\n    let mut line = String::new();\n    io::stdin().read_line(&mut line).unwrap();\n    line.trim().to_owned()\n}\n";
            std::fs::write(repo.join("src/main.rs"), main).unwrap();
            crate::review::tests::run(&repo, &["commit", "-qam", "sample"]);
            for (scheme, tag) in [("Strata Dark", "dark"), ("Strata Light", "light")] {
                let ctx = Context::default();
                let mut app = StrataApp::new(Some(repo.clone()), None);
                app.settings.theme = scheme.into();
                app.settings.minimap = false;
                app.apply_settings();
                shooter.run(&ctx, &mut app, 3, vec![]);
                app.review_start(Some(&ctx));
                for _ in 0..40 {
                    shooter.run(&ctx, &mut app, 1, vec![]);
                    if app.review.reviewing() {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
                // The "agent" at work.
                let edited = main.replace("    let name = read_name();\n", "    let name = read_name().unwrap_or_else(|| \"world\".into());\n").replace(
                    "fn read_name() -> String {\n    let mut line = String::new();\n    io::stdin().read_line(&mut line).unwrap();\n    line.trim().to_owned()\n}\n",
                    "fn read_name() -> Option<String> {\n    let mut line = String::new();\n    io::stdin().read_line(&mut line).ok()?;\n    let name = line.trim();\n    (!name.is_empty()).then(|| name.to_owned())\n}\n",
                );
                std::fs::write(repo.join("src/main.rs"), edited).unwrap();
                std::fs::write(repo.join("README.md"), "# Title\n\nText.\n\nRun it with `cargo run`.\n").unwrap();
                std::fs::write(repo.join("src/args.rs"), "pub fn verbose() -> bool {\n    std::env::args().any(|a| a == \"-v\")\n}\n").unwrap();
                std::fs::remove_file(repo.join("gone.txt")).unwrap();
                app.review.reviewed.clear();
                for _ in 0..80 {
                    shooter.run(&ctx, &mut app, 1, vec![]);
                    if app.review.scan.as_ref().is_some_and(|s| s.files.len() == 4) {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
                app.review_set_reviewed(&repo.join("README.md"), true);
                app.open_review(&repo.join("src/main.rs"));
                // The earlier version is read in the background.
                for _ in 0..80 {
                    if app.review.pending_open.is_none() {
                        break;
                    }
                    shooter.run(&ctx, &mut app, 1, vec![]);
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
                app.review_step_change(true);
                shooter.shoot(&ctx, &mut app, 4, vec![], &dir.join(format!("review_{tag}.ppm")));
                // A narrow window: the file's buttons give way to the summary.
                if tag == "dark"
                    && let Some(mut narrow) = Shooter::new(980, 640, 2.0)
                {
                    let ctx = Context::default();
                    let mut app = StrataApp::new(Some(repo.clone()), None);
                    app.settings.minimap = false;
                    app.apply_settings();
                    app.review_show();
                    for _ in 0..80 {
                        narrow.run(&ctx, &mut app, 1, vec![]);
                        if app.review.scan.is_some() {
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(25));
                    }
                    app.open_review(&repo.join("src/main.rs"));
                    for _ in 0..80 {
                        if app.review.pending_open.is_none() {
                            break;
                        }
                        narrow.run(&ctx, &mut app, 1, vec![]);
                        std::thread::sleep(std::time::Duration::from_millis(25));
                    }
                    narrow.shoot(&ctx, &mut app, 4, vec![], &dir.join("review_narrow.ppm"));
                    // Larger interface text: the heading and Start Review no longer
                    // share a row, so the button moves to a row of its own.
                    app.settings.ui_font_size = 18.0;
                    app.apply_settings();
                    narrow.shoot(&ctx, &mut app, 6, vec![], &dir.join("review_narrow_larger.ppm"));
                }
                // Put things back for the next scheme.
                crate::review::tests::run(&repo, &["checkout", "-q", "--", "."]);
                let _ = std::fs::remove_file(repo.join("src/args.rs"));
            }
            let _ = std::fs::remove_dir_all(&repo);
        }

        // The welcome card on a first launch, dark and light.
        for (scheme, tag) in [("Strata Dark", "dark"), ("Strata Light", "light")] {
            let ctx = Context::default();
            let mut app = StrataApp::new(None, None);
            app.settings.theme = scheme.into();
            app.apply_settings();
            shooter.run(&ctx, &mut app, 3, vec![]);
            app.show_welcome(true);
            shooter.shoot(&ctx, &mut app, 6, vec![], &dir.join(format!("welcome_{tag}.ppm")));
        }
    }
}

/// Window chrome at awkward sizes, to look at by eye:
/// `STRATA_UI_SHOTS=<folder> cargo test --release ui_shots`.
#[cfg(test)]
mod ui_shots {
    use super::*;
    use crate::commands::Cmd;
    use std::path::PathBuf;

    fn tree() -> PathBuf {
        let d = std::env::temp_dir().join("strata-ui-shots-tree");
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("sous-dossier_très_long_nom")).unwrap();
        for n in ["中文文件名_非常长的名字_测试用例.rs", "a_really_really_long_file_name_that_goes_on_and_on.txt", "README.md"] {
            std::fs::write(d.join(n), "# Title\n\nSome text.\n").unwrap();
        }
        d
    }

    #[test]
    fn ui_shots() {
        let Some(dir) = std::env::var_os("STRATA_UI_SHOTS").map(PathBuf::from) else { return };
        let tree = tree();
        let cn = tree.join("中文文件名_非常长的名字_测试用例.rs");
        let long = tree.join("a_really_really_long_file_name_that_goes_on_and_on.txt");
        let md = tree.join("README.md");

        // Many tabs in a narrow window: the current one in view, CJK names, the overflow arrow.
        let mut sh = Shooter::new(640, 420, 2.0).unwrap();
        let ctx = Context::default();
        let mut app = StrataApp::new(Some(long.clone()), None);
        app.settings.theme_mode = "light".into();
        app.apply_settings();
        app.open_folder(tree.clone());
        app.open_path(cn.clone(), false);
        app.open_path(md.clone(), false);
        for n in 0..8 {
            let p = tree.join(format!("extra_file_{n}.txt"));
            std::fs::write(&p, "x\n").unwrap();
            app.open_path(p, false);
        }
        app.open_path(cn.clone(), false);
        sh.shoot(&ctx, &mut app, 6, vec![], &dir.join("tabs_narrow.ppm"));
        app.open_find(true);
        sh.shoot(&ctx, &mut app, 4, vec![Event::Text("Title".into())], &dir.join("find_640.ppm"));

        // The status bar at 420 points.
        let mut sh2 = Shooter::new(420, 300, 2.0).unwrap();
        let ctx = Context::default();
        let mut app = StrataApp::new(Some(long.clone()), None);
        app.settings.theme_mode = "light".into();
        app.apply_settings();
        app.status = "Reloaded a_really_really_long_file_name (changed on disk)".into();
        sh2.shoot(&ctx, &mut app, 5, vec![], &dir.join("status_420.ppm"));
        let mut sh3 = Shooter::new(700, 300, 2.0).unwrap();
        let ctx = Context::default();
        app.docs[0].conflict = true;
        sh3.shoot(&ctx, &mut app, 5, vec![], &dir.join("status_700_conflict.ppm"));

        // Completion near the bottom of the window.
        let ctx = Context::default();
        let p = std::env::temp_dir().join("strata-ui-shots-compl.rs");
        let mut body = String::new();
        for i in 0..60 {
            body += &format!("let identifier_number_{i} = {i};\n");
        }
        body += "ident";
        std::fs::write(&p, &body).unwrap();
        let mut app = StrataApp::new(Some(p), None);
        sh.run(&ctx, &mut app, 3, vec![]);
        let end = app.docs[0].editor.buffer.len_chars();
        app.docs[0].editor.select_ranges(&[end..end], 0);
        app.docs[0].view.reveal();
        sh.run(&ctx, &mut app, 3, vec![]);
        app.run(&ctx, Cmd::ShowCompletions);
        sh.shoot(&ctx, &mut app, 4, vec![], &dir.join("completion_bottom.ppm"));
    }
}

/// The README screenshots (see the module; the scenes need sh and git).
#[cfg(unix)]
mod readme_shots;

/// The interface at each text size, to check legibility by eye:
/// `STRATA_LEGIBILITY_SHOTS=<folder> cargo test --release legibility_shots`
/// writes `<scene>_<size>.ppm` for the default interface size and for
/// `STRATA_LEGIBILITY_SIZES` (default "16,18").
#[cfg(test)]
mod legibility_shots {
    use super::*;
    use crate::settings_meta::Category;
    use std::path::PathBuf;

    fn app(sh: &mut Shooter, ctx: &Context, size: Option<f32>, scheme: &str, path: Option<PathBuf>) -> StrataApp {
        let mut app = StrataApp::new(path, None);
        app.settings.theme_mode = "fixed".into();
        app.settings.theme = scheme.into();
        app.settings.help_in_own_window = false;
        if let Some(s) = size {
            app.settings.ui_font_size = s;
        }
        app.apply_settings();
        sh.run(ctx, &mut app, 3, vec![]);
        app
    }

    fn right_click(at: Pos2) -> Vec<Event> {
        let b = |pressed| Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed, modifiers: egui::Modifiers::NONE };
        vec![Event::PointerMoved(at), b(true), b(false)]
    }

    #[test]
    fn legibility_shots() {
        let Some(dir) = std::env::var_os("STRATA_LEGIBILITY_SHOTS").map(PathBuf::from) else { return };
        std::fs::create_dir_all(&dir).unwrap();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let extra = std::env::var("STRATA_LEGIBILITY_SIZES").unwrap_or_else(|_| "16,18".into());
        let mut sizes: Vec<Option<f32>> = vec![None];
        sizes.extend(extra.split(',').filter_map(|s| s.trim().parse().ok()).map(Some));
        let Some(mut sh) = Shooter::new(1280, 820, 2.0) else { return };
        for size in sizes {
            let tag = size.map_or("default".to_owned(), |s| format!("{s}"));
            let shot = |name: &str| dir.join(format!("{name}_{tag}.ppm"));

            for (scheme, light) in [("Strata Dark", "dark"), ("Strata Light", "light")] {
                let ctx = Context::default();
                let mut a = app(&mut sh, &ctx, size, scheme, Some(root.join("src/diff.rs")));
                a.open_settings_window();
                sh.shoot(&ctx, &mut a, 6, vec![], &shot(&format!("settings_appearance_{light}")));
                a.settings_ui.category_for_shots(Category::Editor);
                sh.shoot(&ctx, &mut a, 4, vec![], &shot(&format!("settings_editor_{light}")));
                a.settings_ui.search = "interface".into();
                sh.shoot(&ctx, &mut a, 4, vec![], &shot(&format!("settings_interface_{light}")));
            }

            // Help, docked.
            let ctx = Context::default();
            let mut a = app(&mut sh, &ctx, size, "Strata Light", Some(root.join("src/diff.rs")));
            a.open_help(Some("find.md"), false);
            sh.shoot(&ctx, &mut a, 6, vec![], &shot("help"));

            // The side bar, tabs and status bar over a folder; then the palette; then a context menu.
            let ctx = Context::default();
            let mut a = app(&mut sh, &ctx, size, "Strata Dark", None);
            a.open_folder(root.join("src/app"));
            a.open_path(root.join("src/app/settings_ui.rs"), false);
            a.open_path(root.join("src/app/help_ui.rs"), false);
            sh.run(&ctx, &mut a, 4, vec![]);
            a.status = "Saved help_ui.rs".into();
            sh.shoot(&ctx, &mut a, 3, vec![], &shot("chrome"));
            a.dev_palette = Some("cmd:toggle".into());
            sh.shoot(&ctx, &mut a, 4, vec![], &shot("palette"));
            sh.run(&ctx, &mut a, 1, vec![Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }]);
            sh.run(&ctx, &mut a, 3, vec![]);
            sh.run(&ctx, &mut a, 1, right_click(egui::pos2(700.0, 300.0)));
            sh.shoot(&ctx, &mut a, 3, vec![], &shot("context_menu"));

            // The update notice, and the welcome card on a first launch.
            let ctx = Context::default();
            let mut a = app(&mut sh, &ctx, size, "Strata Dark", Some(root.join("src/update/version.rs")));
            a.update.updater().set_status(crate::update::Status::Available { release: super::super::update_ui::tests::release(Ok(())), manual: true });
            sh.shoot(&ctx, &mut a, 6, vec![], &shot("update"));
            let ctx = Context::default();
            let mut a = app(&mut sh, &ctx, size, "Strata Light", None);
            a.show_welcome(true);
            sh.shoot(&ctx, &mut a, 6, vec![], &shot("welcome"));

            // The find bar in a narrow window.
            let mut narrow = Shooter::new(640, 420, 2.0).unwrap();
            let ctx = Context::default();
            let mut a = app(&mut narrow, &ctx, size, "Strata Light", Some(root.join("src/diff.rs")));
            a.open_find(true);
            narrow.shoot(&ctx, &mut a, 4, vec![Event::Text("budget".into())], &shot("find_640"));
        }
    }
}
