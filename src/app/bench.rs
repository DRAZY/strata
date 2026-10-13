//! Large-file benchmark (ignored by `cargo test`). Run it in release:
//!
//! ```sh
//! cargo test --release large_file_bench -- --ignored --nocapture --test-threads=1
//! STRATA_BENCH_CASES=50mb-rs,5mb-line cargo test --release large_file_bench -- --ignored --nocapture
//! ```
//!
//! or `scripts/bench-large-files.sh`, which runs each case in its own
//! process so the memory column isn't skewed by earlier cases. The files are
//! generated once into `$TMPDIR/strata-bench` and reused. Every frame runs
//! the whole window (`eframe::App::ui`) headless at 1400×900.

use super::*;
use egui::{Context, Event, Modifiers, RawInput};
use std::time::{Duration, Instant};

struct Case {
    name: &'static str,
    file: &'static str,
    bytes: usize,
    make: fn(usize) -> String,
    /// Raise `large_file_mb` so it opens normally: highlighting, minimap,
    /// bracket colors (as after choosing its syntax in large-file mode).
    full: bool,
}

const MB: usize = 1 << 20;

const CASES: &[Case] = &[
    Case { name: "1mb-rs", file: "one.rs", bytes: MB, make: rust_code, full: false },
    Case { name: "10mb-json", file: "ten.json", bytes: 10 * MB, make: json, full: false },
    Case { name: "50mb-rs", file: "fifty.rs", bytes: 50 * MB, make: rust_code, full: false },
    Case { name: "50mb-rs-full", file: "fifty.rs", bytes: 50 * MB, make: rust_code, full: true },
    Case { name: "100mb-log", file: "hundred.log", bytes: 100 * MB, make: log, full: false },
    Case { name: "100mb-log-full", file: "hundred.log", bytes: 100 * MB, make: log, full: true },
    Case { name: "5mb-line", file: "minified.js", bytes: 5 * MB, make: one_line_js, full: false },
    Case { name: "5mb-line-full", file: "minified.js", bytes: 5 * MB, make: one_line_js, full: true },
    Case { name: "1m-lines", file: "million.txt", bytes: 0, make: short_lines, full: false },
];

/// xorshift: the same files on every run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn pick<'a>(&mut self, words: &[&'a str]) -> &'a str {
        words[(self.next() % words.len() as u64) as usize]
    }
}

const WORDS: &[&str] = &["alpha", "beta", "gamma", "delta", "value", "count", "index", "buffer", "foo", "result", "items", "total"];

fn rust_code(bytes: usize) -> String {
    let mut r = Rng(0x5eed);
    let mut s = String::with_capacity(bytes + 1024);
    let mut n = 0;
    while s.len() < bytes {
        n += 1;
        let (a, b) = (r.pick(WORDS), r.pick(WORDS));
        s.push_str(&format!(
            "/// Computes the {a} of {b} ({n}).\npub fn {a}_{b}_{n}(items: &[u32], limit: usize) -> Option<u64> {{\n    let mut {a} = 0u64;\n    for (i, x) in items.iter().enumerate() {{\n        if i > limit {{ break; }}\n        {a} += u64::from(*x) * {n};\n    }}\n    let text = \"{b} {a} string\";\n    if text.len() > 3 {{ Some({a}) }} else {{ None }}\n}}\n\n"
        ));
    }
    s
}

fn json(bytes: usize) -> String {
    let mut r = Rng(0x1234);
    let mut s = String::with_capacity(bytes + 1024);
    s.push_str("[\n");
    let mut n = 0;
    while s.len() < bytes {
        n += 1;
        s.push_str(&format!(
            "  {{\"id\": {n}, \"name\": \"{} {}\", \"tags\": [\"{}\", \"{}\"], \"score\": {}.{}, \"active\": true, \"nested\": {{\"a\": null, \"b\": [1, 2, 3]}}}},\n",
            r.pick(WORDS),
            r.pick(WORDS),
            r.pick(WORDS),
            r.pick(WORDS),
            r.next() % 1000,
            r.next() % 100
        ));
    }
    s.push_str("  {}\n]\n");
    s
}

fn log(bytes: usize) -> String {
    let mut r = Rng(0xabcdef);
    let mut s = String::with_capacity(bytes + 1024);
    let mut n = 0u64;
    while s.len() < bytes {
        n += 1;
        let level = ["INFO", "DEBUG", "WARN", "ERROR"][(r.next() % 4) as usize];
        s.push_str(&format!(
            "2026-10-13T12:{:02}:{:02}.{:03}Z {level} worker-{} request {} handled {} {} in {}ms\n",
            (n / 60_000) % 60,
            (n / 1000) % 60,
            n % 1000,
            r.next() % 16,
            r.next() % 1_000_000,
            r.pick(WORDS),
            r.pick(WORDS),
            r.next() % 500
        ));
    }
    s
}

fn one_line_js(bytes: usize) -> String {
    let mut r = Rng(0x7777);
    let mut s = String::with_capacity(bytes + 1024);
    let mut n = 0;
    while s.len() < bytes {
        n += 1;
        s.push_str(&format!("function {}{n}(a,b){{var c=a+b*{n};if(c>{}){{return \"{}\"+c}}return c}};", r.pick(WORDS), r.next() % 100, r.pick(WORDS)));
    }
    s.push('\n');
    s
}

fn short_lines(_: usize) -> String {
    let mut r = Rng(0x4242);
    let mut s = String::with_capacity(12 * 1_000_000);
    for n in 0..1_000_000 {
        s.push_str(r.pick(WORDS));
        s.push(' ');
        s.push_str(&(n % 997).to_string());
        s.push('\n');
    }
    s
}

fn bench_dir() -> PathBuf {
    let dir = std::env::temp_dir().join("strata-bench");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn file_for(case: &Case) -> PathBuf {
    let path = bench_dir().join(case.file);
    if !path.exists() {
        std::fs::write(&path, (case.make)(case.bytes)).unwrap();
    }
    path
}

fn rss_mb() -> f64 {
    let out = std::process::Command::new("ps").args(["-o", "rss=", "-p", &std::process::id().to_string()]).output();
    out.ok().and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<f64>().ok()).map_or(0.0, |kb| kb / 1024.0)
}

fn frame(app: &mut StrataApp, ctx: &Context, events: Vec<Event>, modifiers: Modifiers) -> Duration {
    let events = std::iter::once(Event::ModifiersChanged(modifiers)).chain(events).collect();
    let mut raw = RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(1400.0, 900.0))), events, ..Default::default() };
    let t = Instant::now();
    eframe::App::raw_input_hook(app, ctx, &mut raw);
    let mut out = ctx.run_ui(raw, |ui| {
        let mut frame = eframe::Frame::_new_kittest();
        eframe::App::ui(app, ui, &mut frame);
    });
    let took = t.elapsed();
    out.textures_delta.clear();
    took
}

fn idle(app: &mut StrataApp, ctx: &Context) -> Duration {
    frame(app, ctx, Vec::new(), Modifiers::NONE)
}

fn ms(d: Duration) -> String {
    format!("{:.1}", d.as_secs_f64() * 1000.0)
}

/// Profiling aid: `STRATA_BENCH_SPIN=<phase>` repeats that phase for 8 s
/// (attach `sample`/Instruments meanwhile).
fn spin(phase: &str, mut f: impl FnMut()) {
    if std::env::var("STRATA_BENCH_SPIN").is_ok_and(|p| p == phase) {
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(8) {
            f();
        }
    }
}

/// Median and max of `n` idle frames.
fn idle_stats(app: &mut StrataApp, ctx: &Context, n: usize) -> (Duration, Duration) {
    let mut times: Vec<Duration> = (0..n).map(|_| idle(app, ctx)).collect();
    times.sort();
    (times[n / 2], times[n - 1])
}

/// Type one char at `at` (after scrolling there): the typing frame and the
/// slowest of the next few frames (follow-up highlighting, git, word count).
fn type_at(app: &mut StrataApp, ctx: &Context, at: usize) -> (Duration, Duration) {
    let d = &mut app.docs[app.active];
    d.editor.select_ranges(&[at..at], 0);
    d.view.reveal_centered();
    d.view.focus();
    for _ in 0..3 {
        idle(app, ctx);
    }
    let typed = frame(app, ctx, vec![Event::Text("x".into())], Modifiers::NONE);
    let after = (0..5).map(|_| idle(app, ctx)).max().unwrap_or_default();
    (typed, after)
}

fn primary() -> Modifiers {
    if cfg!(target_os = "macos") { Modifiers::MAC_CMD | Modifiers::COMMAND } else { Modifiers::CTRL | Modifiers::COMMAND }
}

fn key(key: Key, modifiers: Modifiers) -> Event {
    Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }
}

fn run_case(case: &Case) -> Vec<(String, String)> {
    let path = file_for(case);
    let size = std::fs::metadata(&path).unwrap().len() as f64 / MB as f64;
    let mut row = vec![("case".to_owned(), format!("{} ({size:.0} MB)", case.name))];
    let mut put = |k: &str, v: String| row.push((k.to_owned(), v));
    let rss0 = rss_mb();
    let ctx = Context::default();
    let mut app = StrataApp::new(None, None);
    idle(&mut app, &ctx);

    let t = Instant::now();
    app.open_path(path.clone(), false);
    put("open", ms(t.elapsed()));
    let d = &mut app.docs[app.active];
    put("mode", if d.large.is_some() { "large file".into() } else { "normal".into() });
    if case.full
        && let Some(large) = d.large.take()
    {
        // Large-file mode off and its syntax chosen: everything on.
        d.highlighter.set_syntax(&large.syntax);
        put("mode", format!("forced normal, {}", large.syntax));
    }
    let first = idle(&mut app, &ctx);
    put("first frame", ms(first));
    let (med, max) = idle_stats(&mut app, &ctx, 30);
    spin("idle", || {
        idle(&mut app, &ctx);
    });
    put("idle med/max", format!("{}/{}", ms(med), ms(max)));
    put("rss after open", format!("{:.0} MB", rss_mb() - rss0));

    let len = app.docs[app.active].editor.buffer.len_chars();
    for (label, at) in [("type start", 0), ("type mid", len / 2), ("type end", len)] {
        let (typed, after) = type_at(&mut app, &ctx, at);
        spin(label, || {
            frame(&mut app, &ctx, vec![Event::Text("x".into())], Modifiers::NONE);
        });
        spin(&format!("{label} idle"), || {
            idle(&mut app, &ctx);
        });
        put(label, format!("{} (+{})", ms(typed), ms(after)));
    }

    // Back to the top, then jump to the end.
    let d = &mut app.docs[app.active];
    d.editor.select_ranges(&[0..0], 0);
    d.view.reveal();
    idle(&mut app, &ctx);
    let end = frame(&mut app, &ctx, vec![key(Key::End, primary())], primary());
    let after = (0..3).map(|_| idle(&mut app, &ctx)).max().unwrap_or_default();
    put("scroll to end", format!("{} (+{})", ms(end), ms(after)));

    let sel = frame(&mut app, &ctx, vec![key(Key::A, primary())], primary());
    spin("selected", || {
        idle(&mut app, &ctx);
    });
    let copy = frame(&mut app, &ctx, vec![Event::Copy], primary());
    let after = (0..3).map(|_| idle(&mut app, &ctx)).max().unwrap_or_default();
    put("select all / copy", format!("{} / {} (+{})", ms(sel), ms(copy), ms(after)));
    let d = &mut app.docs[app.active];
    d.editor.select_ranges(&[0..0], 0);
    idle(&mut app, &ctx);

    app.find.query = crate::find::Query { text: "foo".into(), regex: false, case_sensitive: true, whole_word: false };
    let t = Instant::now();
    app.refresh_matches();
    let n = app.docs[app.active].finder.matches.len();
    put("find foo", format!("{} ({n}{})", ms(t.elapsed()), if app.docs[app.active].finder.truncated { "+" } else { "" }));
    app.find.open = true;
    let with_bar = idle(&mut app, &ctx);
    spin("find", || {
        idle(&mut app, &ctx);
    });
    put("frame w/ find bar", ms(with_bar));
    app.find.open = false;

    // Select all + type: the undo step holds the whole text once.
    let d = &mut app.docs[app.active];
    d.editor.select_all();
    let t = Instant::now();
    d.editor.insert_text("replaced");
    d.editor.undo();
    put("replace all + undo", ms(t.elapsed()));

    let out = bench_dir().join(format!("saved-{}", case.file));
    let t = Instant::now();
    app.docs[app.active].write_to(out.clone()).unwrap();
    put("save", ms(t.elapsed()));
    std::fs::remove_file(out).ok();
    // View ▸ Word Wrap turned on for the file.
    let a = app.active;
    app.docs[a].wrap = Some(true);
    let on = idle(&mut app, &ctx);
    let mid = app.docs[a].editor.buffer.len_chars() / 2;
    let (typed, after) = type_at(&mut app, &ctx, mid);
    put("wrap on / type", format!("{} / {} (+{})", ms(on), ms(typed), ms(after)));
    put("rss end", format!("{:.0} MB", rss_mb() - rss0));
    row
}

/// Hot-exit backup of a dirty 50 MB tab: the frame that writes the session.
fn backup_case() -> Vec<(String, String)> {
    let path = file_for(&CASES[2]);
    let config = bench_dir().join("config");
    let _ = std::fs::remove_dir_all(&config);
    std::fs::create_dir_all(&config).unwrap();
    let ctx = Context::default();
    let mut app = StrataApp::new(None, Some(config.clone()));
    app.open_path(path, false);
    idle(&mut app, &ctx);
    let mut worst = Duration::ZERO;
    let mut total = Duration::ZERO;
    for _ in 0..3 {
        app.docs[app.active].view.focus();
        frame(&mut app, &ctx, vec![Event::Text("y".into())], Modifiers::NONE);
        app.last_session_save = -100.0;
        let f = idle(&mut app, &ctx);
        worst = worst.max(f);
        total += f;
    }
    // Let a background write finish before the next save and the cleanup.
    let t = Instant::now();
    super::autosave::wait();
    let flush = t.elapsed();
    let backups = std::fs::read_dir(config.join("backups")).map(|d| d.count()).unwrap_or(0);
    std::fs::remove_dir_all(&config).ok();
    vec![
        ("case".into(), "50mb-rs dirty, hot-exit backup".into()),
        ("save frame max/avg".into(), format!("{}/{}", ms(worst), ms(total / 3))),
        ("background write".into(), ms(flush)),
        ("backup files".into(), backups.to_string()),
    ]
}

#[test]
#[ignore = "benchmark: run in release with --ignored --nocapture"]
fn large_file_bench() {
    // Per-frame time budgets as in the app (unit tests lift them).
    crate::highlight::UNTIMED.store(false, std::sync::atomic::Ordering::Relaxed);
    let only = std::env::var("STRATA_BENCH_CASES").ok();
    let wanted = |name: &str| only.as_deref().is_none_or(|o| o.split(',').any(|c| c == name));
    for case in CASES.iter().filter(|c| wanted(c.name)) {
        let row = run_case(case);
        println!("\n== {}", row[0].1);
        for (k, v) in &row[1..] {
            println!("  {k:<22} {v}");
        }
    }
    if wanted("backup") {
        let row = backup_case();
        println!("\n== {}", row[0].1);
        for (k, v) in &row[1..] {
            println!("  {k:<22} {v}");
        }
    }
}

/// Frames without a window, kept as images: the app's real output (shapes,
/// tessellated by egui) rasterized on the CPU. For checking a large-file
/// view by eye where no display is available (`STRATA_SCREENSHOT` needs one).
struct Shots {
    ctx: Context,
    textures: std::collections::HashMap<egui::TextureId, egui::ColorImage>,
}

impl Shots {
    fn new() -> Self {
        Self { ctx: Context::default(), textures: Default::default() }
    }

    fn frame(&mut self, app: &mut StrataApp, events: Vec<Event>) -> egui::FullOutput {
        let raw = RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(1400.0, 900.0))), events, ..Default::default() };
        let out = self.ctx.run_ui(raw, |ui| {
            let mut frame = eframe::Frame::_new_kittest();
            eframe::App::ui(app, ui, &mut frame);
        });
        for (id, delta) in out.textures_delta.set.iter().flat_map(|(id, ds)| ds.iter().map(move |d| (id, d))) {
            let egui::ImageData::Color(img) = &delta.image;
            match delta.pos {
                None => {
                    self.textures.insert(*id, (**img).clone());
                }
                Some([x0, y0]) => {
                    let Some(tex) = self.textures.get_mut(id) else { continue };
                    for y in 0..img.size[1] {
                        for x in 0..img.size[0] {
                            tex.pixels[(y0 + y) * tex.size[0] + x0 + x] = img.pixels[y * img.size[0] + x];
                        }
                    }
                }
            }
        }
        out
    }

    /// Run `frames` frames, then save the last as a binary PPM.
    fn save(&mut self, app: &mut StrataApp, frames: usize, path: &Path) {
        let mut out = self.frame(app, Vec::new());
        for _ in 1..frames {
            out = self.frame(app, Vec::new());
        }
        let prims = self.ctx.tessellate(out.shapes, out.pixels_per_point);
        let (w, h) = (1400usize, 900usize);
        let mut px = vec![[0f32; 3]; w * h];
        for p in prims {
            let egui::epaint::Primitive::Mesh(mesh) = p.primitive else { continue };
            let Some(tex) = self.textures.get(&mesh.texture_id) else { continue };
            let clip = p.clip_rect;
            for t in mesh.indices.as_chunks::<3>().0 {
                let [a, b, c] = [t[0], t[1], t[2]].map(|i| mesh.vertices[i as usize]);
                let area = (b.pos.x - a.pos.x) * (c.pos.y - a.pos.y) - (c.pos.x - a.pos.x) * (b.pos.y - a.pos.y);
                if area.abs() < 1e-6 {
                    continue;
                }
                let x0 = a.pos.x.min(b.pos.x).min(c.pos.x).max(clip.min.x).max(0.0).floor() as usize;
                let x1 = (a.pos.x.max(b.pos.x).max(c.pos.x).min(clip.max.x).ceil() as usize).min(w);
                let y0 = a.pos.y.min(b.pos.y).min(c.pos.y).max(clip.min.y).max(0.0).floor() as usize;
                let y1 = (a.pos.y.max(b.pos.y).max(c.pos.y).min(clip.max.y).ceil() as usize).min(h);
                for y in y0..y1 {
                    for x in x0..x1 {
                        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                        let wa = ((b.pos.x - fx) * (c.pos.y - fy) - (c.pos.x - fx) * (b.pos.y - fy)) / area;
                        let wb = ((c.pos.x - fx) * (a.pos.y - fy) - (a.pos.x - fx) * (c.pos.y - fy)) / area;
                        let wc = 1.0 - wa - wb;
                        if wa < -1e-4 || wb < -1e-4 || wc < -1e-4 {
                            continue;
                        }
                        let u = a.uv.x * wa + b.uv.x * wb + c.uv.x * wc;
                        let v = a.uv.y * wa + b.uv.y * wb + c.uv.y * wc;
                        let tx = ((u * tex.size[0] as f32) as usize).min(tex.size[0] - 1);
                        let ty = ((v * tex.size[1] as f32) as usize).min(tex.size[1] - 1);
                        let texel = tex.pixels[ty * tex.size[0] + tx].to_array().map(|c| c as f32 / 255.0);
                        let col = |i: usize| (a.color.to_array()[i] as f32 * wa + b.color.to_array()[i] as f32 * wb + c.color.to_array()[i] as f32 * wc) / 255.0;
                        let alpha = col(3) * texel[3];
                        let dst = &mut px[y * w + x];
                        for (i, d) in dst.iter_mut().enumerate() {
                            *d = col(i) * texel[i] + *d * (1.0 - alpha);
                        }
                    }
                }
            }
        }
        let mut bytes = format!("P6\n{w} {h}\n255\n").into_bytes();
        bytes.extend(px.iter().flat_map(|p| p.map(|c| (c.clamp(0.0, 1.0) * 255.0) as u8)));
        std::fs::write(path, bytes).unwrap();
    }
}

/// `STRATA_SHOTS=dir cargo test --release large_file_screenshots -- --ignored`
/// renders the 50 MB file and the 5 MB one-line file (top, then scrolled far
/// down / right with the caret there) into `dir/*.ppm`.
#[test]
#[ignore = "writes images: run with --ignored and STRATA_SHOTS=dir"]
fn large_file_screenshots() {
    let dir = PathBuf::from(std::env::var("STRATA_SHOTS").unwrap_or_else(|_| bench_dir().display().to_string()));
    for (case, at) in [(&CASES[0], 0.5), (&CASES[2], 0.0), (&CASES[2], 0.73), (&CASES[3], 0.73), (&CASES[6], 0.0), (&CASES[6], 0.5), (&CASES[7], 0.5)] {
        let mut shots = Shots::new();
        let mut app = StrataApp::new(None, None);
        app.open_path(file_for(case), false);
        let d = &mut app.docs[app.active];
        if case.full
            && let Some(large) = d.large.take()
        {
            d.highlighter.set_syntax(&large.syntax);
        }
        let pos = (d.editor.buffer.len_chars() as f64 * at) as usize;
        d.editor.select_ranges(&[pos..pos + 3], 0);
        d.view.reveal_centered();
        d.view.focus();
        let name = format!("{}-{}.ppm", case.name, (at * 100.0) as usize);
        shots.save(&mut app, 8, &dir.join(&name));
        println!("{}  {}", dir.join(name).display(), app.status);
    }
}
