//! Hidden and direction-changing characters (see `hidden`): the status bar
//! notice for a file that has any, and Find ▸ Find Hidden Characters.

use super::StrataApp;
use crate::hidden;

impl StrataApp {
    /// The current tab's count of hidden characters, when it has any.
    pub(super) fn hidden_count(&mut self, ctx: &egui::Context) -> Option<usize> {
        let d = &mut self.docs[self.active];
        if d.binary.is_some() {
            return None;
        }
        let version = d.editor.version();
        d.hidden_count.get(&d.editor.buffer, version, ctx).filter(|&n| n > 0)
    }

    /// Find ▸ Find Hidden Characters: select the next one after the
    /// selection, wrapping around, and say what it is.
    pub(super) fn find_hidden_characters(&mut self) {
        let d = &mut self.docs[self.active];
        let buf = &d.editor.buffer;
        let from = d.editor.primary().end();
        // Stepping from a selected one moves past it, not onto it again.
        let Some(at) = hidden::step(buf, from, true) else {
            self.status = "No hidden or direction-changing characters in this file".into();
            return;
        };
        let (k, n) = (hidden::count(buf.chunks_in(0..at)) + 1, hidden::count(buf.chunks_in(0..buf.len_chars())));
        let what = hidden::describe(buf.char(at)).unwrap_or_default();
        d.editor.select_ranges(&[at..at + 1], 0);
        d.view.reveal_centered();
        self.status = format!("Hidden character {k} of {n}: {what}");
    }
}

/// The status bar notice: short, with the full sentence on hover.
pub(super) fn notice(n: usize) -> (String, String) {
    let s = if n == 1 { "" } else { "s" };
    let label = format!("⚠ {n} hidden character{s}");
    let tip = format!("This file contains {n} hidden or direction-changing character{s}. Click to step through them (Find Hidden Characters).");
    (label, tip)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;
    use crate::editor::Editor;

    fn app_with(text: &str) -> (StrataApp, egui::Context) {
        let mut app = StrataApp::new(None, None);
        app.docs[0].editor = Editor::new(Buffer::from_str(text));
        (app, egui::Context::default())
    }

    #[test]
    fn find_hidden_characters_steps_through_and_wraps() {
        let text = "/*\u{202E} } \u{2066}if (isAdmin)\u{2069} \u{2066} begin admins only */\n";
        let (mut app, _ctx) = app_with(text);
        let at: Vec<usize> = text.chars().enumerate().filter(|(_, c)| hidden::is_hidden(*c)).map(|(i, _)| i).collect();
        assert_eq!(at.len(), 4);
        for (k, &want) in at.iter().chain(&at[..1]).enumerate() {
            app.find_hidden_characters();
            let sel = app.docs[app.active].editor.primary().range();
            assert_eq!(sel, want..want + 1, "step {k}");
            assert!(app.status.starts_with(&format!("Hidden character {} of 4: U+", k % 4 + 1)), "{}", app.status);
        }
        assert!(app.status.contains("RIGHT-TO-LEFT OVERRIDE"), "wrapped back to the first: {}", app.status);
        // Nothing changed in the text.
        assert_eq!(app.docs[app.active].editor.buffer.slice(0..app.docs[app.active].editor.buffer.len_chars()), text);
    }

    /// Every text a frame drew, raw controls included.
    fn drawn(out: &egui::FullOutput) -> Vec<String> {
        fn walk(shape: &egui::Shape, out: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(t) => out.push(t.galley.text().to_owned()),
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
                _ => {}
            }
        }
        let mut texts = Vec::new();
        out.shapes.iter().for_each(|c| walk(&c.shape, &mut texts));
        texts
    }

    #[test]
    fn no_surface_draws_a_raw_hidden_character() {
        let dir = crate::app::ui_tests::temp_dir("hidden-surfaces");
        // A file name that reads "evilrs.png" shown raw, and text that hides a check.
        let name = "evil\u{202E}gnp.rs";
        let path = dir.join(name);
        std::fs::write(&path, "fn main() { let x\u{202E} = 1; }\n").unwrap();
        let ctx = egui::Context::default();
        let mut app = StrataApp::new(None, None);
        app.open_folder(dir.clone());
        app.open_path(path.clone(), false);
        app.status = format!("Saved {name}");
        let mut out = crate::app::ui_tests::frame(&mut app, &ctx, vec![]);
        for _ in 0..3 {
            out = crate::app::ui_tests::frame(&mut app, &ctx, vec![]);
        }
        let texts = drawn(&out);
        let raw: Vec<&String> = texts.iter().filter(|t| hidden::any(t)).collect();
        assert!(raw.is_empty(), "raw hidden characters drawn: {raw:?}");
        let marked = texts.iter().filter(|t| t.contains("evil\u{2009}RLO\u{2009}gnp.rs")).count();
        assert!(marked >= 2, "the tab title and the side bar mark it: {texts:?}");
        assert!(texts.iter().any(|t| t == "Saved evil\u{2009}RLO\u{2009}gnp.rs"), "the status message: {texts:?}");
        // The window title, drawn by the system, spells it out.
        assert!(app.last_title.contains("evil[RLO]gnp.rs"), "{}", app.last_title);
        // Goto Anything lists it marked too.
        app.open_palette(&ctx, crate::palette::Mode::Files, "evil");
        crate::app::ui_tests::frame(&mut app, &ctx, vec![]);
        out = crate::app::ui_tests::frame(&mut app, &ctx, vec![]);
        let texts = drawn(&out);
        assert!(texts.iter().all(|t| !hidden::any(t)), "{texts:?}");
        assert!(texts.iter().any(|t| t.contains("\u{2009}RLO\u{2009}gnp.rs")), "the palette row: {texts:?}");
    }

    /// `cargo test --release hidden_large_file_bench -- --ignored --nocapture`:
    /// typing in a 64 MB file full of hidden characters, and its long line.
    #[test]
    #[ignore = "benchmark: run in release with --ignored --nocapture"]
    fn hidden_large_file_bench() {
        use std::time::{Duration, Instant};
        let dir = crate::app::ui_tests::temp_dir("hidden-bench");
        let line = "    let total = a\u{200B}b + c; /*\u{202E} } \u{2066}if (isAdmin)\u{2069} \u{2066} */ x();\n";
        for (name, text) in [("lines", line.repeat((64 << 20) / line.len())), ("one long line", line.trim_end().repeat((8 << 20) / line.len()))] {
            let path = dir.join(format!("{}.js", name.replace(' ', "_")));
            std::fs::write(&path, &text).unwrap();
            let ctx = egui::Context::default();
            let mut app = StrataApp::new(None, None);
            app.open_path(path, false);
            assert!(app.docs[app.active].large.is_some(), "{name}: large-file mode");
            let mut frames = Vec::new();
            for k in 0..40 {
                let events = if k % 2 == 0 { vec![egui::Event::Text("x".into())] } else { vec![] };
                let t = Instant::now();
                crate::app::ui_tests::frame(&mut app, &ctx, events);
                frames.push(t.elapsed());
            }
            let t = Instant::now();
            let mut n = None;
            while n.is_none() && t.elapsed() < Duration::from_secs(20) {
                crate::app::ui_tests::frame(&mut app, &ctx, vec![]);
                n = app.hidden_count(&ctx);
                std::thread::sleep(Duration::from_millis(2));
            }
            frames.sort();
            println!("{name}: {} MB, frame median {:?}, max {:?}, count {n:?} ready after {:?}", text.len() >> 20, frames[frames.len() / 2], frames[frames.len() - 1], t.elapsed());
            assert!(n.is_some_and(|n| n > 0));
            assert!(frames[frames.len() / 2] < Duration::from_millis(30), "{name}: typing stays fast");
        }
    }

    #[test]
    fn the_notice_counts_and_says_so() {
        let (mut app, ctx) = app_with("plain\n");
        assert_eq!(app.hidden_count(&ctx), None);
        let (mut app2, ctx2) = app_with("a\u{200B}b\u{202E}c\u{00AD}\n");
        assert_eq!(app2.hidden_count(&ctx2), Some(3));
        let (label, tip) = notice(3);
        assert_eq!(label, "⚠ 3 hidden characters");
        assert!(tip.starts_with("This file contains 3 hidden or direction-changing characters."));
        assert_eq!(notice(1).0, "⚠ 1 hidden character");
        app.find_hidden_characters();
        assert_eq!(app.status, "No hidden or direction-changing characters in this file");
    }
}
