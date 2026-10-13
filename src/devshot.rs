//! Dev aid: `STRATA_SCREENSHOT=out.ppm strata file` renders a few frames,
//! saves the window as a PPM image, and quits. `STRATA_SCREENSHOT_DELAY=1.5`
//! waits that many seconds first (for output that arrives later, like a shell's). Works the same on macOS and Windows.

use std::io::Write;

#[derive(Default)]
pub struct DevShot {
    frames: u32,
    requested: bool,
}

impl DevShot {
    pub fn tick(&mut self, ctx: &egui::Context) {
        let Some(path) = std::env::var_os("STRATA_SCREENSHOT") else { return };
        self.frames += 1;
        ctx.request_repaint();
        let delay: f64 = std::env::var("STRATA_SCREENSHOT_DELAY").ok().and_then(|d| d.parse().ok()).unwrap_or(0.0);
        if self.frames >= 10 && ctx.input(|i| i.time) >= delay && !self.requested {
            self.requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
        }
        let image = ctx.input(|i| {
            i.raw.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(image) = image {
            let [w, h] = image.size;
            let mut out = std::io::BufWriter::new(std::fs::File::create(&path).expect("screenshot path"));
            write!(out, "P6\n{w} {h}\n255\n").unwrap();
            for p in &image.pixels {
                out.write_all(&[p.r(), p.g(), p.b()]).unwrap();
            }
            out.flush().unwrap();
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}
