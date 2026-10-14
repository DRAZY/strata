//! Dev aid: `STRATA_SCREENSHOT=out.ppm strata file` renders a few frames,
//! saves the window as a PPM image, and quits. `STRATA_SCREENSHOT_DELAY=1.5`
//! waits that many seconds first (for output that arrives later, like a shell's). Works the same on macOS and Windows.
//! `STRATA_SCREENSHOT_TYPE="te\tx"` types into the editor first, one step per
//! frame: a run of text, or a Tab key for each `\t` (e.g. to show a snippet).

use std::io::Write;

#[derive(Default)]
pub struct DevShot {
    frames: u32,
    requested: bool,
    /// Steps of STRATA_SCREENSHOT_TYPE still to type.
    typing: Option<Vec<egui::Event>>,
    /// Another window takes the screenshot.
    off: bool,
}

impl DevShot {
    pub fn off() -> Self {
        Self { off: true, ..Self::default() }
    }

    /// Returns input to feed to the next frame.
    pub fn tick(&mut self, ctx: &egui::Context) -> Vec<egui::Event> {
        let Some(path) = crate::dev::var("STRATA_SCREENSHOT").filter(|_| !self.off) else { return Vec::new() };
        self.frames += 1;
        ctx.request_repaint();
        let typing = self.typing.get_or_insert_with(|| {
            // A literal backslash-t works too (easier to pass from a shell).
            let text = crate::dev::var("STRATA_SCREENSHOT_TYPE").unwrap_or_default().replace("\\t", "\t");
            let mut steps: Vec<egui::Event> = Vec::new();
            for (i, run) in text.split('\t').enumerate() {
                if i > 0 {
                    let key = egui::Key::Tab;
                    steps.push(egui::Event::Key { key, physical_key: Some(key), pressed: true, repeat: false, modifiers: egui::Modifiers::NONE });
                }
                if !run.is_empty() {
                    steps.push(egui::Event::Text(run.to_owned()));
                }
            }
            steps.reverse();
            steps
        });
        if !typing.is_empty() || crate::dev::var("STRATA_SCREENSHOT_TYPE").is_some() {
            // Launched from a shell the window may stay in the background;
            // act focused so the editor takes the typing (and draws its caret).
            ctx.input_mut(|i| i.focused = true);
        }
        // Give the window a few frames to settle and focus the editor first.
        let step = if self.frames >= 4 && self.frames.is_multiple_of(2) { typing.pop() } else { None };
        let pending = !typing.is_empty();
        let delay: f64 = crate::dev::var("STRATA_SCREENSHOT_DELAY").and_then(|d| d.parse().ok()).unwrap_or(0.0);
        if self.frames >= 10 && !pending && ctx.input(|i| i.time) >= delay && !self.requested {
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
        step.into_iter().collect()
    }
}
