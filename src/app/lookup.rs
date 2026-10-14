//! Searches of the whole folder that would stall the window: Go to
//! Definition's folder pass and Goto Symbol in Project. They read up to
//! thousands of files, so they run on a worker thread; the status bar says
//! "Searching…", and a new request cancels the one still running.

use super::StrataApp;
use super::more::Choosing;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};

/// Files read for a definition, and for the project's symbols.
const DEFINITION_FILES: usize = 20_000;
const SYMBOL_FILES: usize = 5_000;
const SYMBOLS_MAX: usize = 50_000;

enum Found {
    /// Where `word` is defined, if anywhere.
    Definition(String, Option<(PathBuf, usize)>),
    /// Every symbol: (file, 0-based line, name).
    Symbols(Vec<(PathBuf, usize, String)>),
}

#[derive(Default)]
pub(super) struct Lookup {
    job: Option<(Receiver<Found>, Arc<AtomicBool>)>,
}

impl Lookup {
    /// Stop the search in progress (its result is dropped).
    pub fn cancel(&mut self) {
        if let Some((_, stop)) = self.job.take() {
            stop.store(true, Ordering::Relaxed);
        }
    }

    #[cfg(test)]
    pub fn running(&self) -> bool {
        self.job.is_some()
    }

    fn start(&mut self, ctx: Option<egui::Context>, work: impl FnOnce(&AtomicBool) -> Option<Found> + Send + 'static) {
        self.cancel();
        let (tx, rx) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        std::thread::spawn(move || {
            if let Some(found) = work(&flag)
                && !flag.load(Ordering::Relaxed)
            {
                let _ = tx.send(found);
                if let Some(ctx) = ctx {
                    ctx.request_repaint();
                }
            }
        });
        self.job = Some((rx, stop));
    }

    fn poll(&mut self) -> Option<Found> {
        let (rx, _) = self.job.as_ref()?;
        match rx.try_recv() {
            Ok(found) => {
                self.job = None;
                Some(found)
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.job = None;
                None
            }
        }
    }
}

/// Every file under `roots`, respecting .gitignore.
fn files(roots: &[PathBuf]) -> impl Iterator<Item = ignore::DirEntry> {
    let mut b = ignore::WalkBuilder::new(&roots[0]);
    for r in &roots[1..] {
        b.add(r);
    }
    b.require_git(false).build().flatten().filter(|e| e.file_type().is_some_and(|t| t.is_file()))
}

impl StrataApp {
    /// Look for `word`'s definition in the folders' files with extension
    /// `ext`, off the UI thread.
    pub(super) fn find_definition_in_folders(&mut self, ctx: Option<egui::Context>, word: String, syntax: String, ext: std::ffi::OsString) {
        let roots = self.sidebar.roots.clone();
        if roots.is_empty() {
            self.status = format!("No definition found for {word}");
            return;
        }
        self.status = format!("Searching the folder for {word}…");
        self.lookup.start(ctx, move |stop| {
            for entry in files(&roots).take(DEFINITION_FILES) {
                if stop.load(Ordering::Relaxed) {
                    return None;
                }
                let p = entry.path();
                if p.extension() != Some(ext.as_os_str()) || entry.metadata().map_or(true, |m| m.len() > 2_000_000) {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(p) else { continue };
                if !text.contains(&word) {
                    continue;
                }
                if let Some(s) = crate::symbols::symbols(&syntax, &text).into_iter().find(|s| s.name == word) {
                    return Some(Found::Definition(word, Some((p.to_owned(), s.line))));
                }
            }
            Some(Found::Definition(word, None))
        });
    }

    /// Every symbol in the folder's files (bounded), offered in the palette.
    pub(super) fn goto_symbol_in_project(&mut self, ctx: &egui::Context) {
        let roots = self.sidebar.roots.clone();
        if roots.is_empty() {
            return self.run(ctx, crate::commands::Cmd::GotoSymbol);
        }
        self.status = "Searching the folder for symbols…".into();
        self.lookup.start(Some(ctx.clone()), move |stop| {
            let mut found = Vec::new();
            for entry in files(&roots).take(SYMBOL_FILES) {
                if stop.load(Ordering::Relaxed) {
                    return None;
                }
                let p = entry.path();
                let Some(syntax) = crate::highlight::syntax_for_path(p) else { continue };
                if entry.metadata().map_or(true, |m| m.len() > 1_000_000) {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(p) else { continue };
                for s in crate::symbols::symbols(&syntax, &text) {
                    found.push((p.to_owned(), s.line, s.name));
                }
                if found.len() >= SYMBOLS_MAX {
                    found.truncate(SYMBOLS_MAX);
                    break;
                }
            }
            Some(Found::Symbols(found))
        });
    }

    /// A finished search: jump to the definition, or list the symbols.
    pub(super) fn poll_lookup(&mut self, ctx: &egui::Context) {
        match self.lookup.poll() {
            None => {}
            Some(Found::Definition(word, None)) => self.status = format!("No definition found for {word}"),
            Some(Found::Definition(_, Some((path, line)))) => {
                self.status.clear();
                self.push_jump();
                self.open_path(path, false);
                self.goto_line(line);
            }
            Some(Found::Symbols(found)) if found.is_empty() => self.status = "No symbols found in the folder".into(),
            Some(Found::Symbols(found)) => {
                self.status.clear();
                let labels = found.iter().map(|(p, line, name)| format!("{name}  ·  {}:{}", self.sidebar.display_path(p), line + 1)).collect();
                let spots = found.into_iter().map(|(p, line, _)| (p, line)).collect();
                self.choose(ctx, Choosing::Symbol(spots), "Symbol in project", labels);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;
    use crate::document::Document;

    fn wait(app: &mut StrataApp, ctx: &egui::Context) {
        for _ in 0..500 {
            if !app.lookup.running() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
            app.poll_lookup(ctx);
        }
    }

    fn folder(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("strata-lookup-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/lib.rs"), "pub fn far_away() {}\nstruct Thing;\n").unwrap();
        std::fs::write(dir.join("src/main.rs"), "fn main() { far_away(); }\n").unwrap();
        dir
    }

    #[test]
    fn definition_in_the_folder_is_found_in_the_background() {
        let ctx = egui::Context::default();
        let dir = folder("def");
        let mut app = StrataApp::new(None, None);
        app.open_folder(dir.clone());
        app.open_path(dir.join("src/main.rs"), false);
        app.doc().editor.select_ranges(&[14..14], 0);
        app.goto_definition_in(Some(ctx.clone()));
        assert!(app.lookup.running() && app.status.contains("Searching"), "returns at once: {}", app.status);
        wait(&mut app, &ctx);
        assert_eq!(app.doc().path.as_deref(), Some(dir.join("src/lib.rs").as_path()));
        assert_eq!(app.doc().editor.primary().head, 0);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_new_search_cancels_the_old_one() {
        let ctx = egui::Context::default();
        let dir = folder("cancel");
        let mut app = StrataApp::new(None, None);
        app.open_folder(dir.clone());
        app.open_path(dir.join("src/main.rs"), false);
        app.doc().editor.select_ranges(&[14..14], 0);
        app.goto_definition_in(Some(ctx.clone()));
        app.goto_symbol_in_project(&ctx);
        wait(&mut app, &ctx);
        assert_eq!(app.doc().path.as_deref(), Some(dir.join("src/main.rs").as_path()), "the definition jump was cancelled");
        assert!(app.palette.open, "the symbols are listed");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn nothing_found_says_so() {
        let ctx = egui::Context::default();
        let dir = folder("none");
        let mut app = StrataApp::new(None, None);
        app.open_folder(dir.clone());
        app.docs[0] = Document::from_buffer(Some(dir.join("src/new.rs")), Buffer::from_str("nowhere();\n"));
        app.doc().highlighter.set_syntax("Rust");
        app.goto_definition_in(Some(ctx.clone()));
        wait(&mut app, &ctx);
        assert!(app.status.contains("No definition found for nowhere"), "{}", app.status);
        std::fs::remove_dir_all(dir).ok();
    }
}
