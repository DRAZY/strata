//! File ▸ Print… and File ▸ Export ▸ HTML…/PDF…: a small dialog for the
//! options, then the pages (see `crate::print`).
//!
//! Print writes a PDF to a temporary file and opens it in the system viewer
//! (Preview on macOS), where the real print dialog, printer choice and page
//! preview live. That works the same on every platform without a printing
//! toolkit, and what gets printed is exactly what the PDF export saves.

use super::StrataApp;
use crate::print::{self, Line, Options, Paper, Source};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Output {
    /// Open the PDF in the system viewer, to print from there.
    Viewer,
    Pdf,
    Html,
}

/// The dialog, open while `output` is set. Options stick for the session.
pub(super) struct PrintDialog {
    output: Option<Output>,
    opts: Options,
    selection_only: bool,
    /// Markdown: the rendered page (else its source).
    rendered: bool,
}

impl Default for PrintDialog {
    fn default() -> Self {
        Self { output: None, opts: Options::default(), selection_only: false, rendered: true }
    }
}

/// Open `path` with the system's default app for its type.
fn open_with_default_app(path: &Path) -> std::io::Result<()> {
    let mut cmd = if cfg!(target_os = "macos") {
        std::process::Command::new("open")
    } else if cfg!(windows) {
        std::process::Command::new("explorer")
    } else {
        std::process::Command::new("xdg-open")
    };
    cmd.arg(path).spawn().map(|_| ())
}

impl StrataApp {
    /// Print… / Export ▸ PDF… / Export ▸ HTML…: ask for the options first.
    pub(super) fn open_print(&mut self, output: Output) {
        let d = &self.docs[self.active];
        self.print.output = Some(output);
        self.print.selection_only = d.editor.selections().iter().any(|s| !s.is_empty() && d.editor.buffer.line_of(s.start()) != d.editor.buffer.line_of(s.end()));
        self.print.opts.tab_width = d.editor.tab_width;
    }

    fn print_name(&self) -> (String, String) {
        let d = &self.docs[self.active];
        let name = d.path.as_ref().and_then(|p| p.file_name()).map_or_else(|| d.name(), |n| n.to_string_lossy().into_owned());
        let stem = d.path.as_ref().and_then(|p| p.file_stem()).map_or_else(|| "Untitled".to_owned(), |s| s.to_string_lossy().into_owned());
        (name, stem)
    }

    fn is_markdown_doc(&self) -> bool {
        let d = &self.docs[self.active];
        d.compare.is_none() && crate::markdown::is_markdown(d.path.as_deref(), d.highlighter.syntax_name())
    }

    /// The print options dialog; runs the output when confirmed.
    pub(super) fn print_dialog(&mut self, ctx: &egui::Context) {
        let Some(output) = self.print.output else { return };
        let (name, _) = self.print_name();
        let markdown = self.is_markdown_doc();
        let d = &self.docs[self.active];
        let has_selection = d.editor.selections().iter().any(|s| !s.is_empty());
        let (mut go, mut cancel) = (false, false);
        let p = &mut self.print;
        egui::Modal::new(egui::Id::new("print-dialog")).show(ctx, |ui| {
            ui.set_width(340.0);
            let title = match output {
                Output::Viewer => format!("Print {name}"),
                Output::Pdf => format!("Export {name} as PDF"),
                Output::Html => format!("Export {name} as HTML"),
            };
            ui.label(egui::RichText::new(title).strong());
            ui.add_space(6.0);
            if markdown {
                ui.checkbox(&mut p.rendered, "Rendered Markdown (as in the preview)");
            }
            let source = !markdown || !p.rendered;
            ui.checkbox(&mut p.opts.color, "Print in color");
            ui.add_enabled(source, egui::Checkbox::new(&mut p.opts.line_numbers, "Line numbers"));
            ui.add_enabled(source && has_selection, egui::Checkbox::new(&mut p.selection_only, "Selection only"));
            ui.horizontal(|ui| {
                ui.label("Paper:");
                ui.radio_value(&mut p.opts.paper, Paper::Letter, "US Letter");
                ui.radio_value(&mut p.opts.paper, Paper::A4, "A4");
            });
            if output == Output::Viewer {
                ui.label(egui::RichText::new("Opens the pages as a PDF in your PDF viewer; print from there.").small().weak());
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let label = match output {
                    Output::Viewer => "Print…",
                    _ => "Export…",
                };
                go = ui.button(label).clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter));
                cancel = ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape));
            });
        });
        if cancel {
            self.print.output = None;
        } else if go {
            self.print.output = None;
            self.run_print(output);
        }
    }

    /// The current tab (or its selected lines) as highlighted lines.
    fn print_source(&mut self, selection_only: bool) -> Source {
        let (name, _) = self.print_name();
        let d = &mut self.docs[self.active];
        let b = &d.editor.buffer;
        let mut lines = 0..b.len_lines();
        if selection_only && let (Some(a), Some(z)) = (d.editor.selections().iter().filter(|s| !s.is_empty()).map(|s| s.start()).min(), d.editor.selections().iter().map(|s| s.end()).max()) {
            let last = b.line_of(z);
            // A selection ending at a line start doesn't take that line.
            let last = if last > b.line_of(a) && b.line_start(last) == z { last - 1 } else { last };
            lines = b.line_of(a)..last + 1;
        }
        // Drop the empty line after a final newline.
        if lines.end == b.len_lines() && lines.end > lines.start + 1 && b.line_text(lines.end - 1).is_empty() {
            lines.end -= 1;
        }
        let mut hl = crate::highlight::Highlighter::new();
        hl.set_theme(print::PRINT_THEME);
        hl.set_syntax(d.highlighter.syntax_name());
        let mut out = Vec::with_capacity(lines.len());
        let mut start = lines.start;
        while start < lines.end {
            let end = (start + 2_000).min(lines.end) - 1;
            while !hl.prepare(b, start..=end) {}
            for l in start..=end {
                let spans = hl.line(l).map(|v| v.iter().map(|s| (s.range.clone(), s.color, s.italic)).collect()).unwrap_or_default();
                out.push(Line { text: b.line_text(l), spans });
            }
            start = end + 1;
        }
        Source { title: name, date: super::textutil::now_string(), lines: out, first_line: lines.start + 1 }
    }

    /// Lay out and write the pages, then open or save them.
    fn run_print(&mut self, output: Output) {
        let (name, stem) = self.print_name();
        let rendered = self.is_markdown_doc() && self.print.rendered;
        let opts = Options { line_numbers: self.print.opts.line_numbers && !rendered, ..self.print.opts };
        let selection_only = self.print.selection_only && !rendered;
        let html = output == Output::Html;
        let bytes: Vec<u8> = if rendered {
            let md = self.docs[self.active].editor.text_snapshot();
            if html {
                print::markdown_html(&md, &stem, &opts).into_bytes()
            } else {
                print::markdown_pdf(&md, &name, &super::textutil::now_string(), &opts)
            }
        } else {
            let src = self.print_source(selection_only);
            if html { print::code_html(&src, &opts).into_bytes() } else { print::code_pdf(&src, &opts) }
        };
        let ext = if html { "html" } else { "pdf" };
        match output {
            Output::Viewer => {
                let dir = std::env::temp_dir().join("strata-print");
                let path = dir.join(format!("{stem}.{ext}"));
                let written = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, &bytes)).and_then(|()| open_with_default_app(&path));
                self.status = match written {
                    Ok(()) => format!("Opened {name} as a PDF ({} pages) — print it from your PDF viewer", page_count(&bytes)),
                    Err(e) => format!("Couldn't print: {e}"),
                };
            }
            Output::Pdf | Output::Html => {
                let d = &self.docs[self.active];
                let mut dialog = rfd::FileDialog::new().set_file_name(format!("{stem}.{ext}")).add_filter(ext.to_uppercase(), &[ext]);
                if let Some(dir) = d.path.as_ref().and_then(|p| p.parent()) {
                    dialog = dialog.set_directory(dir);
                }
                let Some(out) = dialog.save_file() else { return };
                self.status = write_export(&out, &bytes);
            }
        }
    }
}

fn write_export(out: &PathBuf, bytes: &[u8]) -> String {
    match std::fs::write(out, bytes) {
        Ok(()) => format!("Exported {}", out.display()),
        Err(e) => format!("Couldn't export: {e}"),
    }
}

/// Pages in a PDF this module wrote (for the status line).
fn page_count(pdf: &[u8]) -> usize {
    let text = String::from_utf8_lossy(pdf);
    text.split("/Count ").nth(1).and_then(|s| s.split([' ', '>']).next()).and_then(|n| n.parse().ok()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;
    use crate::editor::Editor;

    #[test]
    fn source_lines_are_highlighted_and_selections_print_their_lines() {
        let mut app = StrataApp::new(None, None);
        app.docs[0].editor = Editor::new(Buffer::from_str("fn main() {\n    let x = 1;\n}\n"));
        app.docs[0].highlighter.set_syntax("Rust");
        let src = app.print_source(false);
        assert_eq!(src.lines.len(), 3, "no empty line after the final newline");
        assert_eq!(src.first_line, 1);
        assert!(src.lines[0].spans.iter().any(|(r, ..)| *r == (0..2)), "`fn` is highlighted: {:?}", src.lines[0].spans);
        app.docs[0].editor.select_ranges(&[16..20], 0);
        let src = app.print_source(true);
        assert_eq!((src.first_line, src.lines.len()), (2, 1));
        assert_eq!(src.lines[0].text, "    let x = 1;");
        let pdf = print::code_pdf(&src, &Options::default());
        assert_eq!(page_count(&pdf), 1);
    }

    /// Writes sample pages for a visual check when STRATA_PRINT_SAMPLE names a folder.
    #[test]
    fn sample_pages() {
        let Some(dir) = std::env::var_os("STRATA_PRINT_SAMPLE").map(PathBuf::from) else { return };
        let mut app = StrataApp::new(Some(PathBuf::from(file!()).canonicalize().unwrap()), None);
        let src = app.print_source(false);
        std::fs::write(dir.join("code.pdf"), print::code_pdf(&src, &Options::default())).unwrap();
        std::fs::write(dir.join("code.html"), print::code_html(&src, &Options::default())).unwrap();
        let sample = "# Release notes\n\nSome *emphasis*, **strong**, ~~gone~~ and `code` with a [link](https://example.com).\n\n## Lists\n\n- one\n- two, which is long enough to wrap onto a second line when it reaches the right margin of the page\n  1. nested\n  2. again\n- [x] done\n- [ ] to do\n\n> [!NOTE]\n> A quoted note that also runs on for a while so that it wraps.\n\n```rust\nfn main() {\n    let greeting = \"héllo\"; // comment\n    println!(\"{greeting}\");\n}\n```\n\n| Key | Value | Notes |\n|---|---|---|\n| a | 1 | first |\n| b | 22 | `second` row |\n\n---\n\nThe end.\n";
        std::fs::write(dir.join("sample.pdf"), print::markdown_pdf(sample, "notes.md", "2026-10-13 09:30", &Options::default())).unwrap();
        let md = std::fs::read_to_string("README.md").unwrap();
        std::fs::write(dir.join("readme.pdf"), print::markdown_pdf(&md, "README.md", "2026-10-13 09:30", &Options::default())).unwrap();
    }
}
