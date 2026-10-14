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

        // The About window over an empty editor.
        let ctx = Context::default();
        let mut app = StrataApp::new(None, None);
        app.about_open = true;
        shooter.shoot(&ctx, &mut app, 4, vec![], &dir.join("about.ppm"));
    }
}
