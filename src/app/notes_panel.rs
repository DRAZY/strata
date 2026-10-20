//! Notes mode on screen (see `notes_ui` for the rest): the backlinks panel
//! under the editor, the sync conflict notice above it, the `[[` completion
//! list, the Notes dialogs (create a missing note, name a note, update links
//! after a rename, trash a conflict copy) and what the Markdown preview
//! shows for `![[embeds]]`.

use super::StrataApp;
use super::notes_ui::{check_note_name, is_image};
use crate::markdown::{Block, Embed, TopBlock};
use crate::notes::index::Vault;
use crate::notes::rename::Rename;
use crate::notes::resolve::{is_note, without_md};
use crate::notes::{self, config::Config, dates, parse};
use crate::search::Stamp;
use egui::{Color32, Key, RichText};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};

/// Embedded images are scaled down to at most this many pixels a side.
const EMBED_PIXELS: u32 = 1600;
/// Images kept decoded (the least recently shown go first).
const EMBED_IMAGES: usize = 24;
/// Larger images and notes aren't embedded.
const EMBED_IMAGE_BYTES: u64 = 64 << 20;
const EMBED_NOTE_BYTES: u64 = 1 << 20;
/// How often an embed's file is checked for changes (seconds).
const EMBED_CHECK_SECS: f64 = 2.0;
const WARN: Color32 = Color32::from_rgb(0xe5, 0x9e, 0x40);

/// What a name dialog is for.
pub(super) enum Naming {
    New,
    FromTemplate(PathBuf),
    Rename(PathBuf),
}

pub(super) enum Dialog {
    /// "Create note “link”?" after following a link to a missing note.
    Create { root: PathBuf, path: PathBuf, link: String },
    Name { root: PathBuf, naming: Naming, name: String, error: Option<String> },
    /// Update links after a rename (or look for them first).
    Links(LinkPlan),
    /// Move a sync conflict copy to the Trash: the copy as it was when asked.
    TrashCopy { root: PathBuf, path: PathBuf, original: PathBuf, expected: crate::claim::Expected },
}

impl Dialog {
    pub fn name(root: PathBuf, naming: Naming, initial: &str) -> Dialog {
        Dialog::Name { root, naming, name: initial.to_owned(), error: None }
    }

    /// The vault the dialog is about (kept indexed while it's up).
    pub fn root(&self) -> Option<PathBuf> {
        match self {
            Dialog::Create { root, .. } | Dialog::Name { root, .. } | Dialog::TrashCopy { root, .. } => Some(root.clone()),
            Dialog::Links(p) => Some(p.root.clone()),
        }
    }
}

/// A file a rename's link update changes.
pub(super) struct PlanFile {
    pub path: PathBuf,
    pub rel: String,
    /// The file as read (None: it's open in a tab and changes there).
    pub stamp: Option<Stamp>,
    pub in_tab: bool,
    /// An open tab's text size: Undo Rename keeps it before and after the update.
    pub tab_bytes: usize,
    pub count: usize,
    /// (0-based line, before, after).
    pub lines: Vec<(usize, String, String)>,
}

pub(super) struct LinkPlan {
    pub root: PathBuf,
    from_rel: String,
    to_rel: String,
    pub rename: Option<Arc<Rename>>,
    pub files: Vec<PlanFile>,
    /// Files with links that can't be read or rewritten safely, and why.
    skipped: Vec<(PathBuf, String)>,
    /// Waiting for the vault to be indexed, or for the files to be read.
    pub waiting: bool,
    /// The files being read and their links worked out, off the UI thread.
    pub reading: Option<Receiver<Planned>>,
    /// Files whose old text is too large to keep for Undo Rename.
    pub too_large: usize,
}

/// The files a link update changes, and those it can't (with why).
pub(super) type Planned = (Vec<PlanFile>, Vec<(PathBuf, String)>);

impl LinkPlan {
    pub fn waiting(root: PathBuf, from_rel: String, to_rel: String) -> LinkPlan {
        LinkPlan { root, from_rel, to_rel, rename: None, files: Vec::new(), skipped: Vec::new(), waiting: true, reading: None, too_large: 0 }
    }

    pub fn rename_from(&self) -> &str {
        &self.from_rel
    }

    pub fn rename_to(&self) -> &str {
        &self.to_rel
    }

    pub fn ready(&mut self, rename: Arc<Rename>, files: Vec<PlanFile>, skipped: Vec<(PathBuf, String)>, limit: usize) {
        // The open tabs' texts (before and after) first, then the disk job's
        // originals in order, while they fit.
        let mut kept = 0u64;
        self.too_large = 0;
        for f in files.iter().filter(|f| f.in_tab) {
            let len = 2 * f.tab_bytes as u64;
            if kept + len <= limit as u64 {
                kept += len;
            } else {
                self.too_large += 1;
            }
        }
        for f in files.iter().filter(|f| !f.in_tab) {
            let len = f.stamp.map_or(0, |s| s.len);
            if kept + len <= limit as u64 {
                kept += len;
            } else {
                self.too_large += 1;
            }
        }
        self.rename = Some(rename);
        self.files = files;
        self.skipped = skipped;
        self.waiting = false;
        self.reading = None;
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn skipped_count(&self) -> usize {
        self.skipped.len()
    }
}

// ---- embeds in the preview --------------------------------------------------------------

enum ImageState {
    Loading(Receiver<Result<egui::ColorImage, String>>),
    Ready(egui::TextureHandle),
    Failed(String),
}

struct CachedImage {
    stamp: Option<Stamp>,
    checked: f64,
    /// Last shown (input time).
    used: f64,
    state: ImageState,
}

struct CachedNote {
    stamp: Option<Stamp>,
    checked: f64,
    title: String,
    blocks: Arc<Vec<TopBlock>>,
}

/// Images and notes the preview embeds, read once and checked for changes
/// every couple of seconds. Images are decoded off the UI thread.
#[derive(Default)]
pub(super) struct EmbedCache {
    images: RefCell<HashMap<PathBuf, CachedImage>>,
    notes: RefCell<HashMap<(PathBuf, String), CachedNote>>,
}

fn decode_image(path: &Path) -> Result<egui::ColorImage, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let img = image::load_from_memory(&bytes).map_err(|e| e.to_string())?;
    let img = if img.width() > EMBED_PIXELS || img.height() > EMBED_PIXELS { img.thumbnail(EMBED_PIXELS, EMBED_PIXELS) } else { img };
    let rgba = img.to_rgba8();
    Ok(egui::ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw()))
}

impl EmbedCache {
    fn image(&self, ctx: &egui::Context, path: &Path) -> Embed {
        let now = ctx.input(|i| i.time);
        let mut images = self.images.borrow_mut();
        let stale = images.get(path).is_none_or(|c| now - c.checked > EMBED_CHECK_SECS && Stamp::of(path).ok() != c.stamp);
        if stale {
            while images.len() >= EMBED_IMAGES {
                let Some(oldest) = images.iter().min_by(|a, b| a.1.used.total_cmp(&b.1.used)).map(|(p, _)| p.clone()) else { break };
                images.remove(&oldest);
            }
            let stamp = Stamp::of(path).ok();
            let state = if stamp.is_some_and(|s| s.len > EMBED_IMAGE_BYTES) {
                ImageState::Failed("too large to show".into())
            } else {
                let (tx, rx) = std::sync::mpsc::channel();
                let (p, ctx) = (path.to_owned(), ctx.clone());
                std::thread::spawn(move || {
                    let _ = tx.send(decode_image(&p));
                    ctx.request_repaint();
                });
                ImageState::Loading(rx)
            };
            images.insert(path.to_owned(), CachedImage { stamp, checked: now, used: now, state });
        }
        let Some(c) = images.get_mut(path) else { return Embed::Loading };
        c.used = now;
        if now - c.checked > EMBED_CHECK_SECS {
            c.checked = now;
        }
        if let ImageState::Loading(rx) = &c.state {
            match rx.try_recv() {
                Ok(Ok(img)) => c.state = ImageState::Ready(ctx.load_texture(format!("note-embed:{}", path.display()), img, egui::TextureOptions::LINEAR)),
                Ok(Err(e)) => {
                    log::warn!("couldn't show {}: {e}", path.display());
                    c.state = ImageState::Failed(e);
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => c.state = ImageState::Failed("couldn't read it".into()),
            }
        }
        let name = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        match &c.state {
            ImageState::Loading(_) => Embed::Loading,
            ImageState::Ready(tex) => Embed::Image(egui::load::SizedTexture::from_handle(tex)),
            ImageState::Failed(why) => Embed::Link(format!("{name} ({why})")),
        }
    }

    fn note(&self, ctx: &egui::Context, path: &Path, sub: &str) -> Embed {
        let now = ctx.input(|i| i.time);
        let key = (path.to_owned(), sub.to_owned());
        let mut notes = self.notes.borrow_mut();
        if let Some(c) = notes.get_mut(&key) {
            if now - c.checked <= EMBED_CHECK_SECS {
                return Embed::Note { title: c.title.clone(), blocks: c.blocks.clone() };
            }
            c.checked = now;
            if Stamp::of(path).ok() == c.stamp {
                return Embed::Note { title: c.title.clone(), blocks: c.blocks.clone() };
            }
        }
        let name = path.file_stem().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let stamp = Stamp::of(path).ok();
        if stamp.is_some_and(|s| s.len > EMBED_NOTE_BYTES) {
            return Embed::Link(format!("{name} (too large to embed)"));
        }
        let text = match crate::search::read_text(path) {
            Ok(f) => f.text,
            Err(e) => return Embed::Link(format!("{name} ({e})")),
        };
        let all = crate::markdown::parse_note(&text);
        let blocks = if sub.is_empty() {
            all
        } else {
            match section(&all, &text, sub) {
                Some(b) => b,
                None => return Embed::Link(format!("{name}#{sub} (not found)")),
            }
        };
        let title = if sub.is_empty() { name } else { format!("{name} › {sub}") };
        if notes.len() > 256 {
            notes.clear();
        }
        let blocks = Arc::new(blocks);
        notes.insert(key, CachedNote { stamp, checked: now, title: title.clone(), blocks: blocks.clone() });
        Embed::Note { title, blocks }
    }
}

/// The blocks of one heading's section (to the next heading as high), or
/// the block holding `^id`.
fn section(blocks: &[TopBlock], text: &str, sub: &str) -> Option<Vec<TopBlock>> {
    if let Some(id) = sub.strip_prefix('^') {
        let line = parse::parse(text).blocks.into_iter().find(|(b, _)| b.eq_ignore_ascii_case(id))?.1;
        let k = blocks.iter().rposition(|b| b.line <= line)?;
        return Some(vec![blocks[k].clone()]);
    }
    let want = parse::heading_key(sub.rsplit('#').next().unwrap_or(sub));
    let level_of = |b: &TopBlock| match &b.block {
        Block::Heading { level, .. } => Some(*level),
        _ => None,
    };
    let k = blocks.iter().position(|b| matches!(&b.block, Block::Heading { text, .. } if parse::heading_key(&text.text) == want))?;
    let level = level_of(&blocks[k])?;
    let end = blocks[k + 1..].iter().position(|b| level_of(b).is_some_and(|l| l <= level)).map_or(blocks.len(), |e| k + 1 + e);
    Some(blocks[k..end].to_vec())
}

/// `path` as it is now, for trashing exactly this file later (see `claim`).
fn expected_file(path: &Path) -> Option<crate::claim::Expected> {
    let mut f = crate::claim::open_plain(path).ok()?;
    let meta = f.metadata().ok()?;
    let hash = crate::claim::fingerprint_file(&mut f)?;
    Some(crate::claim::Expected { id: crate::claim::file_id(path), len: meta.len(), mtime: meta.modified().ok(), hash })
}

/// The notes index as the preview of one note sees it.
pub(super) struct PreviewNotes<'a> {
    pub vault: Arc<Vault>,
    pub from: PathBuf,
    /// The vault has been indexed (until then no link counts as missing).
    pub ready: bool,
    pub cache: &'a EmbedCache,
}

impl crate::markdown::Notes for PreviewNotes<'_> {
    fn resolves(&self, target: &str) -> bool {
        let path = target.split('#').next().unwrap_or("");
        !self.ready || path.trim().is_empty() || self.vault.resolve(path, &self.from).is_some()
    }

    fn embed(&self, ctx: &egui::Context, target: &str, wiki: bool, depth: usize) -> Embed {
        let (path, sub) = target.split_once('#').unwrap_or((target, ""));
        let (path, sub) = if wiki { (path.to_owned(), sub.to_owned()) } else { (parse::percent_decode(path), parse::percent_decode(sub)) };
        if crate::links::scheme(&path).is_some() {
            // Never fetched: Strata makes no network connections for notes.
            return Embed::Link(target.to_owned());
        }
        let Some(t) = self.vault.resolve(&path, &self.from) else {
            return if self.ready { Embed::Link(format!("{path} (not found)")) } else { Embed::Loading };
        };
        let abs = self.vault.path_of(t);
        if is_image(&abs) {
            return self.cache.image(ctx, &abs);
        }
        // Notes embed one level deep, and never themselves.
        if !is_note(self.vault.files.path(t)) || depth >= 1 || abs == self.from {
            return Embed::Link(target.to_owned());
        }
        self.cache.note(ctx, &abs, &sub)
    }
}

// ---- the window ------------------------------------------------------------------------

impl StrataApp {
    /// The preview's view of the notes index for the current tab, if it's a note.
    pub(super) fn preview_notes(&mut self) -> Option<(Arc<Vault>, PathBuf, bool)> {
        let root = self.note_root(self.active)?;
        let vault = self.notes.index.vault(&root)?.clone();
        Some((vault, self.docs[self.active].path.clone()?, self.notes.index.ready(&root)))
    }

    /// A link clicked in the preview of a note.
    pub(super) fn preview_note_link(&mut self, link: &str, markdown: bool) {
        let Some(from) = self.docs[self.active].path.clone() else { return };
        let (path, sub) = link.split_once('#').unwrap_or((link, ""));
        let (path, sub) = if markdown { (parse::percent_decode(path), parse::percent_decode(sub)) } else { (path.to_owned(), sub.to_owned()) };
        self.follow_note_link(&from, &path, (!sub.is_empty()).then_some(sub.as_str()));
    }

    /// Is the backlinks panel up for the current tab?
    pub(super) fn backlinks_shown(&self) -> bool {
        let d = &self.docs[self.active];
        let Some(path) = d.path.as_deref().filter(|_| crate::markdown::is_markdown(d.path.as_deref(), d.highlighter.syntax_name()) && d.compare.is_none()) else { return false };
        let Some(vault) = self.notes.index.vault_for(path) else { return false };
        match self.settings.notes_backlinks.as_str() {
            "never" => false,
            "always" => true,
            // A folder used for notes: an Obsidian vault, or notes with wikilinks.
            _ => vault.config.obsidian || vault.wikilinks,
        }
    }

    /// The backlinks panel under the editor.
    pub(super) fn notes_panel(&mut self, ui: &mut egui::Ui) {
        if self.distraction.is_some() || !self.backlinks_shown() {
            return;
        }
        let Some(path) = self.docs[self.active].path.clone() else { return };
        let Some(vault) = self.notes.index.vault_for(&path).cloned() else { return };
        let ready = self.notes.index.ready(&vault.root);
        let me = vault.find(&path);
        let mut open: Option<(PathBuf, usize)> = None;
        let mut tag_clicked: Option<String> = None;
        let (mut hide, mut undo) = (false, false);
        let can_undo = self.notes.last_rename.as_ref().is_some_and(super::notes_ui::UndoRename::undoable) && !self.notes.disk.running();
        let size = self.ui_size();
        egui::Panel::bottom(self.link.id("notes-backlinks")).resizable(true).default_size((size * 9.0).max(120.0)).min_size(44.0).show(ui, |ui| {
            let links: &[(u32, u32)] = me.map_or(&[], |i| vault.backlinks(i));
            let mut sources: Vec<u32> = links.iter().map(|&(n, _)| n).collect();
            sources.dedup();
            // The buttons on the right first, so tags that don't fit can't push them out.
            ui.horizontal(|ui| ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                hide = ui.small_button("×").on_hover_text("Hide backlinks (Notes: Toggle Backlinks shows them again)").clicked();
                if can_undo {
                    undo = ui.small_button("Undo Rename").on_hover_text("Put the links back as they were, and the old name").clicked();
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.set_clip_rect(ui.max_rect().intersect(ui.clip_rect()));
                    ui.label(RichText::new("BACKLINKS").small().weak());
                    if ready {
                        let s = |n: usize| if n == 1 { "" } else { "s" };
                        ui.label(RichText::new(format!("{} link{} from {} note{}", links.len(), s(links.len()), sources.len(), s(sources.len()))).small());
                    } else {
                        ui.spinner();
                        ui.label(RichText::new("Indexing notes…").small().weak());
                    }
                    if let Some(note) = me.and_then(|i| vault.note(i)) {
                        let mut seen: Vec<String> = Vec::new();
                        for (t, _) in &note.tags {
                            if seen.iter().any(|s| s.eq_ignore_ascii_case(t)) {
                                continue;
                            }
                            seen.push(t.clone());
                            if ui.small_button(format!("#{t}")).on_hover_text("Notes with this tag").clicked() {
                                tag_clicked = Some(t.clone());
                            }
                        }
                    }
                });
            }));
            ui.separator();
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                if ready && links.is_empty() {
                    ui.label(RichText::new("No other notes link here yet.").weak());
                }
                let mono = egui::TextStyle::Small.resolve(ui.style());
                for src in sources {
                    let rel = vault.files.path(src);
                    crate::fonts::ensure_cjk(ui.ctx(), rel);
                    let src_path = vault.path_of(src);
                    if ui.add(egui::Label::new(RichText::new(crate::hidden::plain(without_md(rel)).into_owned()).strong()).sense(egui::Sense::click())).on_hover_text(src_path.display().to_string()).clicked() {
                        open = Some((src_path.clone(), 0));
                    }
                    for &(n, k) in links.iter().filter(|(n, _)| *n == src) {
                        let Some(l) = vault.note(n).and_then(|note| note.links.get(k as usize)) else { continue };
                        crate::fonts::ensure_cjk(ui.ctx(), &l.context);
                        let mut job = egui::text::LayoutJob::default();
                        job.append(&format!("{:>5}  ", l.line + 1), 0.0, egui::TextFormat::simple(mono.clone(), ui.visuals().weak_text_color()));
                        crate::hidden::append(&mut job, &l.context, 0.0, egui::TextFormat::simple(mono.clone(), ui.visuals().text_color()));
                        // One row per link, cut short with … (the tooltip has it all).
                        let resp = ui.add(egui::Label::new(job).truncate().sense(egui::Sense::click()));
                        if resp.hovered() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        if resp.clicked() {
                            open = Some((src_path.clone(), l.line as usize));
                        }
                    }
                }
            });
        });
        if hide {
            self.settings.notes_backlinks = "never".into();
            self.persist_settings();
        }
        if undo {
            self.run(&ui.ctx().clone(), crate::commands::Cmd::NotesUndoRename);
        }
        if let Some(t) = tag_clicked {
            self.choose_tag(&ui.ctx().clone(), Some(&t));
        }
        if let Some((p, line)) = open {
            self.open_at(p, line, 0..0);
        }
    }

    /// Above the editor: this note has a sync conflict copy, or is one.
    pub(super) fn notes_conflict_bar(&mut self, ui: &mut egui::Ui) {
        if self.distraction.is_some() {
            return;
        }
        let Some(path) = self.docs[self.active].path.clone() else { return };
        let Some(vault) = self.notes.index.vault_for(&path).cloned() else { return };
        let (copies, original) = match vault.find(&path) {
            Some(i) => (vault.copies(i).to_vec(), vault.original_of(i)),
            None => (Vec::new(), None),
        };
        // Files an interrupted undo set aside, anywhere in the vault.
        let leftovers: Vec<crate::swap::Leftover> = vault.leftovers.iter().filter_map(|s| crate::swap::leftover(s)).collect();
        if copies.is_empty() && original.is_none() && leftovers.is_empty() {
            return;
        }
        let name = |k: u32| vault.files.path(k).rsplit('/').next().unwrap_or("").to_owned();
        let mut act: Option<(u8, PathBuf, PathBuf)> = None;
        egui::Panel::top(self.link.id("notes-conflict")).show(ui, |ui| {
            if let Some(o) = original {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(WARN, "⚠");
                    ui.label(crate::hidden::plain(&format!("This is a sync conflict copy of “{}”: another version, left by a sync tool.", name(o))).into_owned());
                    let op = vault.path_of(o);
                    if ui.small_button("Compare with Original").clicked() {
                        act = Some((0, op.clone(), path.clone()));
                    }
                    if ui.small_button("Open Original").clicked() {
                        act = Some((1, op.clone(), path.clone()));
                    }
                    if ui.small_button("Move This Copy to Trash…").clicked() {
                        act = Some((2, path.clone(), op));
                    }
                });
            }
            for left in &leftovers {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(WARN, "⚠");
                    ui.label(left.describe());
                    if left.sealed && ui.small_button("Put Back").clicked() {
                        act = Some((3, left.stage.clone(), path.clone()));
                    }
                    if ui.small_button(crate::ctxmenu::reveal_label()).clicked() {
                        act = Some((4, left.stage.clone(), path.clone()));
                    }
                });
            }
            for c in copies {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(WARN, "⚠");
                    ui.label(crate::hidden::plain(&format!("Sync conflict: “{}” is another version of this note.", name(c))).into_owned());
                    let cp = vault.path_of(c);
                    if ui.small_button("Compare").on_hover_text("Side by side; copy what you want to keep into this note").clicked() {
                        act = Some((0, cp.clone(), path.clone()));
                    }
                    if ui.small_button("Open Copy").clicked() {
                        act = Some((1, cp.clone(), path.clone()));
                    }
                    if ui.small_button("Move Copy to Trash…").clicked() {
                        act = Some((2, cp, path.clone()));
                    }
                });
            }
        });
        match act {
            Some((0, other, _)) => self.compare_conflict(Some(other)),
            Some((1, other, _)) => self.open_path(other, false),
            Some((3, stage, _)) => {
                self.status = match crate::swap::put_back_leftover(&vault.root, &stage) {
                    Ok(to) => format!("Put back {}", to.display()),
                    Err(e) => format!("Couldn't put it back: {e}"),
                };
                self.notes.index.request(&vault.root, Some(ui.ctx()));
            }
            Some((4, stage, _)) => {
                if let Err(e) = crate::ctxmenu::reveal_in_file_manager(&stage) {
                    self.status = format!("Could not open the file manager: {e}");
                }
            }
            Some((_, copy, _)) => self.ask_trash_copy(&copy),
            None => {}
        }
    }

    /// Ask before moving conflict copy `copy` to the Trash.
    pub(super) fn ask_trash_copy(&mut self, copy: &Path) {
        let Some(vault) = self.notes.index.vault_for(copy).cloned() else { return };
        let Some(i) = vault.find(copy) else { return };
        let Some(original) = vault.original_of(i).or_else(|| vault.copies(i).first().copied()).map(|o| vault.path_of(o)) else { return };
        let (copy, original) = if vault.original_of(i).is_some() { (copy.to_owned(), original) } else { (original, copy.to_owned()) };
        // Exactly this file goes, or none: a copy replaced while the question is up stays.
        let Some(expected) = expected_file(&copy) else {
            self.status = format!("Couldn't read {}", copy.display());
            return;
        };
        self.notes.dialog = Some(Dialog::TrashCopy { root: vault.root.clone(), path: copy, original, expected });
    }

    /// The `[[` completion list under the caret.
    pub(super) fn notes_complete_show(&mut self, ctx: &egui::Context) {
        let d = &self.docs[self.active];
        let Some(c) = self.notes.complete.as_ref().filter(|c| c.view == d.view.id && !c.items.is_empty()) else { return };
        let Some(caret) = d.view.caret_rect() else { return };
        let font = egui::FontId::proportional(self.ui_size());
        let small = egui::FontId::proportional(crate::fonts::secondary_size(self.ui_size()));
        let row_h = font.size + 10.0;
        let screen = ctx.content_rect();
        let wanted = c.items.len().min(10) as f32 * (row_h + 2.0) + 16.0;
        let (below, above) = (screen.bottom() - caret.bottom() - 4.0, caret.top() - screen.top() - 4.0);
        let up = wanted > below && above > below;
        let room = if up { above } else { below };
        let (pivot, at) = if up { (egui::Align2::LEFT_BOTTOM, caret.left_top() + egui::vec2(-4.0, -2.0)) } else { (egui::Align2::LEFT_TOP, caret.left_bottom() + egui::vec2(-4.0, 2.0)) };
        let mut chosen = None;
        let selected = c.selected;
        egui::Area::new(egui::Id::new("note-link-completions")).order(egui::Order::Foreground).pivot(pivot).fixed_pos(at).constrain(false).show(ctx, |ui| {
            egui::Frame::popup(ui.style()).inner_margin(4.0).show(ui, |ui| {
                egui::ScrollArea::vertical().max_height((room - 16.0).clamp(row_h, row_h * 10.0 + 8.0)).show(ui, |ui| {
                    let width = (screen.width() * 0.45).clamp(260.0, 560.0);
                    for (i, item) in c.items.iter().enumerate() {
                        crate::fonts::ensure_cjk(ui.ctx(), &item.label);
                        crate::fonts::ensure_cjk(ui.ctx(), &item.detail);
                        let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, row_h), egui::Sense::click());
                        if i == selected {
                            ui.painter().rect_filled(rect, 3.0, ui.visuals().selection.bg_fill);
                        }
                        let label = ui.painter().layout_no_wrap(item.label.clone(), font.clone(), ui.visuals().text_color());
                        let label_w = label.size().x;
                        ui.painter().galley(rect.left_center() + egui::vec2(8.0, -label.size().y / 2.0), label, ui.visuals().text_color());
                        if !item.detail.is_empty() {
                            let mut job = egui::text::LayoutJob::simple_singleline(item.detail.clone(), small.clone(), ui.visuals().weak_text_color());
                            job.wrap.max_width = (rect.width() - label_w - 32.0).max(40.0);
                            job.wrap.max_rows = 1;
                            job.wrap.break_anywhere = true;
                            let g = ui.painter().layout_job(job);
                            ui.painter().galley(rect.right_center() + egui::vec2(-8.0 - g.size().x, -g.size().y / 2.0), g, ui.visuals().weak_text_color());
                        }
                        if resp.clicked() {
                            chosen = Some(i);
                        }
                        if i == selected {
                            resp.scroll_to_me(None);
                        }
                    }
                });
            });
        });
        if let Some(i) = chosen {
            self.notes_accept(i);
            self.docs[self.active].view.focus();
        }
    }

    /// The Notes dialogs.
    pub(super) fn notes_dialogs(&mut self, ctx: &egui::Context) {
        let Some(dialog) = self.notes.dialog.take() else { return };
        let (enter, esc) = ctx.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
        let keep = match dialog {
            Dialog::Create { root, path, link } => {
                let (mut yes, mut no) = (false, false);
                egui::Modal::new(egui::Id::new("notes-create")).show(ctx, |ui| {
                    ui.set_width(380.0);
                    crate::fonts::ensure_cjk(ui.ctx(), &link);
                    ui.label(RichText::new(crate::hidden::plain(&format!("Create “{link}”?")).into_owned()).strong());
                    ui.add_space(4.0);
                    let shown = path.strip_prefix(&root).unwrap_or(&path).display().to_string();
                    ui.label(RichText::new(format!("There's no note called that yet. It would be a new, empty note: {shown}")).small().weak());
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        yes = ui.button("Create Note").clicked() || enter;
                        no = ui.button("Cancel").clicked() || esc;
                    });
                });
                if yes {
                    // Jump Back returns to the link.
                    self.push_jump();
                    self.create_and_open(&root, &path, "");
                }
                (!yes && !no).then_some(Dialog::Create { root, path, link })
            }
            Dialog::Name { root, naming, mut name, mut error } => {
                let (mut ok, mut cancel) = (false, false);
                let title = match &naming {
                    Naming::New => "New note".to_owned(),
                    Naming::FromTemplate(t) => format!("New note from “{}”", t.file_stem().map_or_else(String::new, |s| s.to_string_lossy().into_owned())),
                    Naming::Rename(p) => format!("Rename “{}” to", p.file_stem().map_or_else(String::new, |s| s.to_string_lossy().into_owned())),
                };
                egui::Modal::new(egui::Id::new("notes-name")).show(ctx, |ui| {
                    ui.set_width(360.0);
                    ui.label(RichText::new(title).strong());
                    crate::fonts::ensure_cjk(ui.ctx(), &name);
                    let id = egui::Id::new("notes-name-field");
                    let field = ui.add(egui::TextEdit::singleline(&mut name).id(id).desired_width(f32::INFINITY));
                    // The field keeps the keyboard; the name it opens with is
                    // selected, ready to be typed over.
                    if !field.has_focus() {
                        crate::ctxmenu::focus_selecting_all(ui.ctx(), id, &name);
                    }
                    if let Some(e) = &error {
                        ui.colored_label(Color32::LIGHT_RED, e);
                    }
                    if matches!(naming, Naming::Rename(_)) {
                        ui.label(RichText::new("Links to it in other notes can be updated next.").small().weak());
                    }
                    ui.horizontal(|ui| {
                        ok = ui.button("OK").clicked() || enter;
                        cancel = ui.button("Cancel").clicked() || esc;
                    });
                });
                if cancel {
                    None
                } else if ok {
                    match self.finish_naming(&root, &naming, &name) {
                        Ok(()) => None,
                        Err(e) => {
                            error = Some(e);
                            Some(Dialog::Name { root, naming, name, error })
                        }
                    }
                } else {
                    Some(Dialog::Name { root, naming, name, error })
                }
            }
            Dialog::Links(plan) => self.links_dialog(ctx, plan, enter, esc),
            Dialog::TrashCopy { root, path, original, expected } => {
                let (mut yes, mut no) = (false, false);
                let trash = crate::trash::trash_name();
                egui::Modal::new(egui::Id::new("notes-trash-copy")).show(ctx, |ui| {
                    ui.set_width(380.0);
                    let name = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                    crate::fonts::ensure_cjk(ui.ctx(), &name);
                    ui.label(RichText::new(crate::hidden::plain(&format!("Move “{name}” to the {trash}?")).into_owned()).strong());
                    ui.add_space(4.0);
                    let of = original.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                    ui.label(RichText::new(crate::hidden::plain(&format!("It's a sync conflict copy of “{of}”. Compare them first if you haven't; you can put it back from the {trash}.")).into_owned()).small().weak());
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        yes = ui.button(format!("Move to {trash}")).clicked() || enter;
                        no = ui.button("Cancel").clicked() || esc;
                    });
                });
                if yes {
                    use crate::claim::Refused;
                    match crate::claim::trash_listed(&root, &path, None, &expected) {
                        Ok(()) => {
                            self.follow_delete(&path);
                            self.link.requests.deleted.push(path.clone());
                            self.status = format!("Moved {} to the {trash}", path.display());
                        }
                        Err(Refused::Changed) => self.status = format!("{} changed since you asked, so it wasn't moved to the {trash}", path.display()),
                        Err(Refused::KeptAt(p)) => self.status = format!("{} changed since you asked; it's kept at {}", path.display(), p.display()),
                        Err(Refused::Outside) => self.status = format!("{} isn't inside the vault, so it wasn't moved", path.display()),
                        Err(Refused::OldBack) => self.status = format!("{} wasn't moved", path.display()),
                        Err(Refused::Failed(e)) => self.status = format!("Could not move {} to the {trash}: {e}", path.display()),
                    }
                    self.notes.index.request(&root, Some(ctx));
                }
                (!yes && !no).then_some(Dialog::TrashCopy { root, path, original, expected })
            }
        };
        // A dialog may have opened another (rename → update links).
        if self.notes.dialog.is_none() {
            self.notes.dialog = keep;
        }
    }

    /// OK in a name dialog.
    fn finish_naming(&mut self, root: &Path, naming: &Naming, name: &str) -> Result<(), String> {
        let name = name.trim();
        check_note_name(name)?;
        match naming {
            Naming::Rename(from) => return self.rename_note(from, name),
            Naming::New | Naming::FromTemplate(_) => {}
        }
        let config = Config::read(root);
        let from = self.docs[self.active].path.clone();
        let path = self.new_note_path(root, &config, from.as_deref(), name)?;
        if std::fs::symlink_metadata(&path).is_ok() {
            return Err(format!("{} already exists", path.strip_prefix(root).unwrap_or(&path).display()));
        }
        let text = match naming {
            Naming::FromTemplate(t) => {
                let template = super::notes_ui::read_vault_text(root, t).map_err(|e| format!("Couldn't read the template: {e}"))?;
                let title = name.rsplit('/').next().unwrap_or(name);
                notes::fill_template(&template, title, &dates::DateTime::now(), &config.templates.date_format, &config.templates.time_format)
            }
            _ => String::new(),
        };
        self.create_and_open(root, &path, &text);
        Ok(())
    }

    /// "Update links?" with every change listed.
    fn links_dialog(&mut self, ctx: &egui::Context, plan: LinkPlan, enter: bool, esc: bool) -> Option<Dialog> {
        let (mut yes, mut no) = (false, false);
        let from_name = without_md(plan.from_rel.rsplit('/').next().unwrap_or(&plan.from_rel)).to_owned();
        let to_name = without_md(plan.to_rel.rsplit('/').next().unwrap_or(&plan.to_rel)).to_owned();
        let size = self.ui_size();
        egui::Modal::new(egui::Id::new("notes-update-links")).show(ctx, |ui| {
            ui.set_width((ctx.content_rect().width() - 80.0).clamp(320.0, 720.0));
            crate::fonts::ensure_cjk(ui.ctx(), &from_name);
            crate::fonts::ensure_cjk(ui.ctx(), &to_name);
            ui.label(RichText::new("Update links?").strong());
            ui.add_space(4.0);
            if plan.waiting {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(crate::hidden::plain(&format!("Looking for links to “{from_name}”…")).into_owned());
                });
                no = ui.button("Don't Update").clicked() || esc;
                return;
            }
            let links: usize = plan.files.iter().map(|f| f.count).sum();
            let s = |n: usize| if n == 1 { "" } else { "s" };
            ui.label(crate::hidden::plain(&format!("“{from_name}” is now “{to_name}”. Update {links} link{} in {} note{} to the new name?", s(links), plan.files.len(), s(plan.files.len()))).into_owned());
            let in_tabs = plan.files.iter().filter(|f| f.in_tab).count();
            let mut note = String::new();
            if in_tabs > 0 {
                note += &format!("{in_tabs} open note{} will change in {}, unsaved. ", s(in_tabs), if in_tabs == 1 { "its tab" } else { "their tabs" });
            }
            if plan.too_large > 0 {
                let n = plan.too_large;
                ui.colored_label(WARN, format!("{n} file{} too large to undo, so this rename can't be undone.", if n == 1 { " is" } else { "s are" }));
            } else {
                note += "Notes: Undo Rename puts everything back.";
            }
            ui.label(RichText::new(note.trim_end()).small().weak());
            ui.add_space(4.0);
            let mono = egui::FontId::monospace((size - 1.0).max(10.0));
            egui::ScrollArea::vertical().max_height((ctx.content_rect().height() * 0.5).max(160.0)).auto_shrink([false, true]).show(ui, |ui| {
                for f in &plan.files {
                    crate::fonts::ensure_cjk(ui.ctx(), &f.rel);
                    ui.label(RichText::new(crate::hidden::plain(&f.rel).into_owned()).strong());
                    for (line, old, new) in &f.lines {
                        crate::fonts::ensure_cjk(ui.ctx(), old);
                        crate::fonts::ensure_cjk(ui.ctx(), new);
                        for (mark, text, color) in [("−", old, ui.visuals().weak_text_color()), ("+", new, ui.visuals().text_color())] {
                            let mut job = egui::text::LayoutJob::default();
                            let head = if mark == "−" { format!("{:>5} {mark} ", line + 1) } else { format!("{:>5} {mark} ", "") };
                            job.append(&head, 0.0, egui::TextFormat::simple(mono.clone(), ui.visuals().weak_text_color()));
                            crate::hidden::append(&mut job, text.trim(), 0.0, egui::TextFormat::simple(mono.clone(), color));
                            job.wrap.max_rows = 1;
                            job.wrap.max_width = ui.available_width();
                            ui.label(job);
                        }
                    }
                }
                if !plan.skipped.is_empty() {
                    ui.add_space(4.0);
                    ui.colored_label(WARN, format!("{} file{} can't be updated:", plan.skipped.len(), s(plan.skipped.len())));
                    for (p, why) in &plan.skipped {
                        ui.label(RichText::new(format!("{}: {why}", p.display())).small().weak());
                    }
                }
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                yes = ui.button("Update Links").clicked() || enter;
                no = ui.button("Don't Update").clicked() || esc;
            });
        });
        if yes && !plan.waiting {
            self.apply_link_plan(plan, Some(ctx.clone()));
            return None;
        }
        (!no).then_some(Dialog::Links(plan))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_of_a_note_for_embeds() {
        let text = "# Top\nintro\n## A\na text ^blk\n### A1\ndeep\n## B\nb text\n";
        let blocks = crate::markdown::parse_note(text);
        let heads = |v: &[TopBlock]| v.iter().filter_map(|b| if let Block::Heading { text, .. } = &b.block { Some(text.text.clone()) } else { None }).collect::<Vec<_>>();
        assert_eq!(heads(&section(&blocks, text, "A").unwrap()), ["A", "A1"]);
        assert_eq!(heads(&section(&blocks, text, "Top#B").unwrap()), ["B"], "nested: the last part");
        let blk = section(&blocks, text, "^blk").unwrap();
        assert!(matches!(&blk[0].block, Block::Paragraph(p) if p.text.starts_with("a text")));
        assert!(section(&blocks, text, "Nope").is_none());
    }
}
