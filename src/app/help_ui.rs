//! Help ▸ Documentation: the manual (see `help`) in its own native window
//! (beside the editor, to read along while trying things) or docked as a
//! window over the editor, with contents, search, back/forward and links
//! between pages.
//!
//! While the help has the keyboard, Esc closes it (or clears the search),
//! Find focuses its search box, Close File closes it, and Alt+←/→ go back
//! and forward.

use super::StrataApp;
use crate::help::{self, Hit, Link};
use crate::keybindings::Keymap;
use crate::markdown::{DocRender, TopBlock};
use egui::{Color32, Key, RichText, Sense, vec2};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

thread_local! {
    /// The last page read, shared by every window for the session.
    static LAST_PAGE: RefCell<&'static str> = const { RefCell::new(help::WELCOME) };
}

pub struct HelpState {
    pub open: bool,
    /// The help window has the keyboard (opened, or clicked last).
    pub active: bool,
    page: &'static str,
    back: Vec<(&'static str, f32)>,
    forward: Vec<(&'static str, f32)>,
    search: String,
    /// The search results are showing instead of the page.
    results: bool,
    focus_search: bool,
    /// The search box had the keyboard last frame (egui drops focus on Esc
    /// before we see the key).
    search_focused: bool,
    focus_content: bool,
    /// A heading to scroll to once the page has been laid out.
    pending_anchor: Option<String>,
    /// Frames the anchor has been held: layout can still shift (fonts load).
    anchor_frames: u8,
    scroll_to: Option<f32>,
    scroll: f32,
    /// Parsed pages, for the keymap they were made with.
    blocks: Option<(&'static str, *const Keymap, Arc<Vec<TopBlock>>)>,
    index: Option<(*const Keymap, Rc<Vec<help::Section>>)>,
    hits: Option<(String, Vec<Hit>)>,
    headings: Vec<(String, f32)>,
    render: DocRender,
    /// The window's rect last frame, to tell clicks inside from outside.
    rect: Option<egui::Rect>,
    /// Showing in its own native window rather than over the editor.
    pub detached: bool,
    /// The toolbar's Pop Out / Dock button was pressed.
    toggle_detached: bool,
    /// Where the own window first opened (kept, so the builder doesn't move it).
    position: Option<egui::Pos2>,
}

impl Default for HelpState {
    fn default() -> Self {
        Self {
            open: false,
            active: false,
            page: help::WELCOME,
            back: Vec::new(),
            forward: Vec::new(),
            search: String::new(),
            results: false,
            focus_search: false,
            search_focused: false,
            focus_content: false,
            pending_anchor: None,
            anchor_frames: 0,
            scroll_to: None,
            scroll: 0.0,
            blocks: None,
            index: None,
            hits: None,
            headings: Vec::new(),
            render: DocRender::default(),
            rect: None,
            detached: false,
            toggle_detached: false,
            position: None,
        }
    }
}

impl HelpState {
    /// Show `page` (at `anchor`), remembering where we were for Back.
    pub fn go(&mut self, page: &str, anchor: Option<String>) {
        let Some(p) = help::page(page) else { return };
        if p.id != self.page || self.results {
            self.back.push((self.page, self.scroll));
            self.forward.clear();
        }
        self.page = p.id;
        LAST_PAGE.with(|l| *l.borrow_mut() = p.id);
        self.results = false;
        self.scroll_to = Some(0.0);
        self.pending_anchor = anchor;
        self.anchor_frames = 0;
        self.focus_content = true;
    }

    fn step(&mut self, back: bool) {
        let (from, to) = if back { (&mut self.back, &mut self.forward) } else { (&mut self.forward, &mut self.back) };
        if let Some((page, y)) = from.pop() {
            to.push((self.page, self.scroll));
            self.page = page;
            LAST_PAGE.with(|l| *l.borrow_mut() = page);
            self.results = false;
            self.scroll_to = Some(y);
            self.pending_anchor = None;
        }
    }

    #[cfg(test)]
    pub fn page(&self) -> &'static str {
        self.page
    }

    fn page_blocks(&mut self) -> Arc<Vec<TopBlock>> {
        let keymap = Rc::as_ptr(&crate::keybindings::current());
        match &self.blocks {
            Some((p, k, b)) if *p == self.page && *k == keymap => b.clone(),
            _ => {
                let text = help::page(self.page).map(|p| p.text()).unwrap_or_default();
                let b = Arc::new(crate::markdown::parse(&text));
                self.blocks = Some((self.page, keymap, b.clone()));
                b
            }
        }
    }

    fn hits(&mut self) -> &[Hit] {
        let keymap = Rc::as_ptr(&crate::keybindings::current());
        let index = match &self.index {
            Some((k, i)) if *k == keymap => i.clone(),
            _ => {
                let i = Rc::new(help::sections());
                self.index = Some((keymap, i.clone()));
                self.hits = None;
                i
            }
        };
        if self.hits.as_ref().is_none_or(|(q, _)| *q != self.search) {
            self.hits = Some((self.search.clone(), help::search(&index, &self.search)));
        }
        &self.hits.as_ref().expect("just set").1
    }
}

impl StrataApp {
    /// Open the help at `page` (None: the last page read this session).
    pub(super) fn open_help(&mut self, page: Option<&str>, search: bool) {
        let h = &mut self.help;
        let page = page.unwrap_or_else(|| LAST_PAGE.with(|l| *l.borrow()));
        if !h.open {
            h.detached = self.settings.help_in_own_window;
        }
        if !h.open || h.page != page {
            h.go(page, None);
        }
        h.open = true;
        h.active = true;
        h.focus_content = !search;
        h.focus_search = search;
        if search {
            h.results = !h.search.is_empty();
        }
    }

    /// Dev aid (STRATA_DEV_PALETTE="help:find.md#goto-anything" or
    /// "helpsearch:query", with STRATA_SCREENSHOT).
    pub(super) fn dev_help(&mut self, spec: &str, search: bool) {
        if search {
            self.help.search = spec.to_owned();
            self.open_help(None, true);
        } else {
            let (page, anchor) = spec.split_once('#').map_or((spec, None), |(p, a)| (p, Some(a.to_owned())));
            self.open_help(Some(page), false);
            self.help.pending_anchor = anchor;
        }
    }

    fn close_help(&mut self) {
        self.help.open = false;
        self.help.active = false;
        self.doc().view.focus();
    }

    /// Commands that mean something else while the help has the keyboard.
    /// Returns true when the help took the command.
    pub(super) fn help_takes(&mut self, cmd: crate::commands::Cmd) -> bool {
        use crate::commands::Cmd;
        if !(self.help.open && self.help.active) {
            return false;
        }
        match cmd {
            Cmd::Find | Cmd::IncrementalFind | Cmd::FindInFiles => self.help.focus_search = true,
            Cmd::CloseTab => self.close_help(),
            _ => return false,
        }
        true
    }

    /// Keys for the help, before the keymap sees them.
    /// `ctx` input is the help's own window when `own_window` (it has the
    /// keyboard while focused), else the editor window's.
    pub(super) fn help_keys(&mut self, ctx: &egui::Context) {
        self.help_keys_in(ctx, false);
    }

    fn help_keys_in(&mut self, ctx: &egui::Context, own_window: bool) {
        if !self.help.open || self.help_in_own_window(ctx) != own_window {
            return;
        }
        if own_window {
            self.help.active = ctx.input(|i| i.viewport().focused.unwrap_or(false));
        } else {
            // A click inside the window gives it the keyboard; one outside takes it away.
            let press = ctx.input(|i| i.events.iter().find_map(|e| match e {
                egui::Event::PointerButton { pos, pressed: true, .. } => Some(*pos),
                _ => None,
            }));
            if let (Some(p), Some(r)) = (press, self.help.rect) {
                self.help.active = r.contains(p);
            }
        }
        if !self.help.active || self.palette.open {
            return;
        }
        let search_focused = self.help.search_focused || ctx.memory(|m| m.has_focus(search_id()));
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape)) {
            if search_focused && !self.help.search.is_empty() {
                self.help.search.clear();
                self.help.results = false;
                self.help.focus_search = true;
            } else if self.help.results {
                self.help.results = false;
                self.help.focus_content = true;
            } else {
                self.close_help();
            }
        }
        if !search_focused {
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::ALT, Key::ArrowLeft)) {
                self.help.step(true);
            }
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::ALT, Key::ArrowRight)) {
                self.help.step(false);
            }
        }
    }

    /// The help window, when open.
    pub(super) fn help_window(&mut self, ctx: &egui::Context) {
        if !self.help.open {
            return;
        }
        if std::mem::take(&mut self.help.toggle_detached) {
            self.help.detached = !self.help.detached;
            self.help.active = true;
            self.help.focus_content = true;
            self.settings.help_in_own_window = self.help.detached;
            self.persist_settings();
        }
        if self.help_in_own_window(ctx) {
            self.help_own_window(ctx);
            return;
        }
        let mut open = true;
        let screen = ctx.content_rect();
        let size = vec2((screen.width() - 80.0).clamp(320.0, 1040.0), (screen.height() - 90.0).clamp(240.0, 820.0));
        let title = help::page(self.help.page).map_or_else(String::new, |p| p.title());
        let hl = &self.docs[self.active].highlighter;
        let look = Look { font_size: (self.settings.font_size + 1.0).clamp(11.0, 22.0), ui_size: self.settings.ui_font_size.clamp(9.0, 24.0), ids: [self.link.id("help-toolbar"), self.link.id("help-toc")] };
        let id = self.link.id("help-window");
        let help = &mut self.help;
        let mut status = None;
        let resp = egui::Window::new(format!("Strata Help — {title}"))
            .id(id)
            .open(&mut open)
            .collapsible(false)
            .default_size(size)
            .min_size(vec2(320.0, 200.0))
            .max_size(screen.size() - vec2(16.0, 16.0))
            .default_pos(screen.center() - size / 2.0)
            .constrain(true)
            .resizable(true)
            .frame(egui::Frame::window(&ctx.global_style()).fill(hl.background()).inner_margin(0))
            .show(ctx, |ui| status = contents(help, ui, hl, &look));
        if let Some(s) = status {
            self.status = s;
        }
        if let Some(r) = resp {
            self.help.rect = Some(r.response.rect);
        }
        if !open {
            self.close_help();
        }
    }

    /// Showing in a window of its own: chosen, and native windows exist
    /// (without them, as in tests, it can only dock).
    fn help_in_own_window(&self, ctx: &egui::Context) -> bool {
        self.help.detached && !ctx.embed_viewports()
    }

    /// The help in a native window of its own, first placed beside the editor window.
    fn help_own_window(&mut self, ctx: &egui::Context) {
        let title = help::page(self.help.page).map_or_else(String::new, |p| p.title());
        let size = vec2(880.0, 760.0);
        if self.help.position.is_none() {
            // To the right of the editor window, or its top-left corner when there's no room.
            let main = ctx.input(|i| i.viewport().outer_rect);
            let monitor = ctx.input(|i| i.viewport().monitor_size);
            self.help.position = Some(match (main, monitor) {
                (Some(r), Some(m)) if r.right() + 12.0 + size.x <= m.x => r.right_top() + vec2(12.0, 0.0),
                (Some(r), _) => r.left_top() + vec2(40.0, 40.0),
                _ => egui::pos2(80.0, 80.0),
            });
        }
        let mut builder = egui::ViewportBuilder::default()
            .with_title(format!("Strata Help — {title}"))
            .with_inner_size(size)
            .with_min_inner_size(vec2(360.0, 240.0))
            .with_icon(eframe::icon_data::from_png_bytes(include_bytes!("../../assets/icon-app.png")).expect("bundled icon"));
        if let Some(p) = self.help.position {
            builder = builder.with_position(p);
        }
        let id = egui::ViewportId::from_hash_of(self.link.id("help-own-window"));
        let look = Look { font_size: (self.settings.font_size + 1.0).clamp(11.0, 22.0), ui_size: self.settings.ui_font_size.clamp(9.0, 24.0), ids: [self.link.id("help-toolbar"), self.link.id("help-toc")] };
        let mut status = None;
        let mut closed = false;
        ctx.show_viewport_immediate(id, builder, |ui, _| {
            let ctx = ui.ctx().clone();
            if ctx.input(|i| i.viewport().close_requested()) {
                closed = true;
                return;
            }
            self.help_keys_in(&ctx, true);
            if !self.help.open {
                return;
            }
            // Keys the editor window's menu would otherwise take (on macOS the
            // menu bar sends them to the focused editor window: see `help_takes`).
            if !cfg!(target_os = "macos") {
                if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::F)) {
                    self.help.focus_search = true;
                }
                if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::W)) {
                    closed = true;
                    return;
                }
            }
            let hl = &self.docs[self.active].highlighter;
            let help = &mut self.help;
            egui::CentralPanel::default().frame(egui::Frame::new().fill(hl.background())).show(ui, |ui| status = contents(help, ui, hl, &look));
        });
        if let Some(s) = status {
            self.status = s;
        }
        if closed {
            self.close_help();
        }
    }
}

/// Sizes and panel ids for drawing the help.
struct Look {
    font_size: f32,
    ui_size: f32,
    ids: [egui::Id; 2],
}

/// The inside of the help window. Returns a status-bar message, if any.
fn contents(h: &mut HelpState, ui: &mut egui::Ui, hl: &crate::highlight::Highlighter, look: &Look) -> Option<String> {
    let (font_size, ui_size) = (look.font_size, look.ui_size);
    {
        let bg = hl.background();
        let fg = ui.visuals().text_color();
        let dim = fg.gamma_multiply(0.6);
        let accent = hl.accent();
        let panel = ui.visuals().panel_fill;
        let rule = ui.visuals().widgets.noninteractive.bg_stroke;
        ui.set_min_size(ui.available_size());

        // ---- toolbar: back, forward, search
        egui::Panel::top(look.ids[0]).frame(egui::Frame::new().fill(panel).inner_margin(egui::Margin::symmetric(10, 7)).stroke(rule)).show(ui, |ui| {
            ui.horizontal(|ui| {
                let nav = |ui: &mut egui::Ui, glyph: &str, tip: &str, enabled: bool| ui.add_enabled(enabled, egui::Button::new(RichText::new(glyph).size(ui_size + 2.0)).frame(false).min_size(vec2(26.0, 24.0))).on_hover_text(tip).clicked();
                if nav(ui, "←", "Back (Alt+←)", !h.back.is_empty()) {
                    h.step(true);
                }
                if nav(ui, "→", "Forward (Alt+→)", !h.forward.is_empty()) {
                    h.step(false);
                }
                ui.add_space(6.0);
                let find = crate::commands::Cmd::Find.shortcut_label().unwrap_or_default();
                let edit = egui::TextEdit::singleline(&mut h.search)
                    .id(search_id())
                    .hint_text(format!("Search the documentation   {find}"))
                    .font(egui::FontId::proportional(ui_size))
                    .background_color(bg)
                    .desired_width(ui.available_width().min(420.0))
                    .margin(egui::Margin::symmetric(8, 5));
                let r = ui.add(edit);
                if h.focus_search {
                    h.focus_search = false;
                    r.request_focus();
                    if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), search_id()) {
                        state.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(h.search.chars().count()))));
                        state.store(ui.ctx(), search_id());
                    }
                }
                if r.changed() {
                    h.results = !h.search.trim().is_empty();
                }
                h.search_focused = r.has_focus();
                if r.has_focus() {
                    h.active = true;
                }
                // Enter opens the best result.
                if r.lost_focus()
                    && ui.input(|i| i.key_pressed(Key::Enter))
                    && !h.search.trim().is_empty()
                    && let Some(hit) = h.hits().first().cloned()
                {
                    h.go(hit.page, hit.slug);
                }
                // Pop out into a window of its own, or dock back over the editor.
                if !ui.ctx().embed_viewports() {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let (label, tip) = if h.detached {
                            ("Dock", "Show the documentation inside the editor window")
                        } else {
                            ("Pop Out", "Open the documentation in its own window, to read along while you work")
                        };
                        if ui.add(egui::Button::new(RichText::new(label).size(ui_size)).min_size(vec2(0.0, 24.0))).on_hover_text(tip).clicked() {
                            h.toggle_detached = true;
                        }
                    });
                }
            });
        });

        // ---- contents
        let toc_width = (ui_size * 15.0).clamp(150.0, 220.0);
        let mut goto: Option<&'static str> = None;
        egui::Panel::left(look.ids[1])
            .resizable(false)
            .exact_size(toc_width)
            .frame(egui::Frame::new().fill(panel).inner_margin(egui::Margin { left: 8, right: 6, top: 8, bottom: 8 }).stroke(rule))
            .show(ui, |ui| {
                egui::ScrollArea::vertical().id_salt("help-toc-scroll").auto_shrink(false).show(ui, |ui| {
                    let mut section = "";
                    for p in help::PAGES {
                        if p.section != section {
                            section = p.section;
                            ui.add_space(if ui.min_rect().height() > 1.0 { 10.0 } else { 2.0 });
                            ui.label(RichText::new(section.to_uppercase()).size(ui_size * 0.78).color(dim).strong());
                            ui.add_space(2.0);
                        }
                        let current = p.id == h.page && !h.results;
                        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ui_size + 12.0), Sense::click());
                        if current || resp.hovered() {
                            ui.painter().rect_filled(rect, 4.0, if current { mix(panel, accent, 0.18) } else { mix(panel, fg, 0.06) });
                        }
                        if current {
                            ui.painter().rect_filled(egui::Rect::from_min_size(rect.min + vec2(0.0, 4.0), vec2(2.5, rect.height() - 8.0)), 1.0, accent);
                        }
                        let color = if current { fg } else { fg.gamma_multiply(0.82) };
                        let galley = ui.painter().layout(p.title(), egui::FontId::proportional(ui_size), color, rect.width() - 14.0);
                        ui.painter().galley(egui::pos2(rect.left() + 9.0, rect.center().y - galley.size().y / 2.0), galley, color);
                        if resp.clicked() {
                            goto = Some(p.id);
                        }
                        resp.on_hover_cursor(egui::CursorIcon::PointingHand);
                    }
                });
            });
        if let Some(p) = goto {
            h.go(p, None);
        }

        // ---- the page, or search results
        let content_id = content_id();
        let mut clicked: Option<String> = None;
        let mut open_hit: Option<Hit> = None;
        egui::CentralPanel::default().frame(egui::Frame::new().fill(bg)).show(ui, |ui| {
            let resp = ui.interact(ui.max_rect(), content_id, Sense::click());
            if h.focus_content {
                h.focus_content = false;
                resp.request_focus();
            }
            if resp.clicked() {
                resp.request_focus();
            }
            let focused = resp.has_focus();
            if h.results {
                let hits = h.hits().to_vec();
                egui::ScrollArea::vertical().id_salt("help-results").auto_shrink(false).show(ui, |ui| {
                    egui::Frame::new().inner_margin(egui::Margin::symmetric(20, 14)).show(ui, |ui| {
                        let n = hits.len();
                        let label = match n {
                            0 => format!("No results for “{}”", h.search.trim()),
                            1 => "1 result".to_owned(),
                            n => format!("{n} results"),
                        };
                        ui.label(RichText::new(label).size(ui_size).color(dim));
                        ui.add_space(6.0);
                        for hit in hits {
                            let w = ui.available_width();
                            let title = if hit.slug.is_some() { format!("{}  ›  {}", hit.page_title, hit.heading) } else { hit.page_title.clone() };
                            let tg = ui.painter().layout(title, egui::FontId::proportional(ui_size + 1.5), accent, w - 16.0);
                            let sg = ui.painter().layout(hit.snippet.clone(), egui::FontId::proportional(ui_size), fg.gamma_multiply(0.75), w - 16.0);
                            let h = tg.size().y + sg.size().y + 18.0;
                            let (rect, r) = ui.allocate_exact_size(vec2(w, h), Sense::click());
                            if r.hovered() {
                                ui.painter().rect_filled(rect, 6.0, mix(bg, fg, 0.06));
                            }
                            let p = rect.min + vec2(8.0, 7.0);
                            let th = tg.size().y;
                            ui.painter().galley(p, tg, accent);
                            ui.painter().galley(p + vec2(0.0, th + 3.0), sg, fg);
                            if r.clicked() {
                                open_hit = Some(hit);
                            }
                            r.on_hover_cursor(egui::CursorIcon::PointingHand);
                        }
                    });
                });
                return;
            }
            let blocks = h.page_blocks();
            let mut area = egui::ScrollArea::vertical().id_salt(("help-page", h.page)).auto_shrink(false);
            if focused {
                let page = ui.max_rect().height() * 0.9;
                let line = font_size * 3.0;
                let delta = ui.input_mut(|i| {
                    let mut d = 0.0;
                    for (k, amount) in [(Key::ArrowDown, line), (Key::ArrowUp, -line), (Key::PageDown, page), (Key::PageUp, -page), (Key::Space, page), (Key::Home, -1e7), (Key::End, 1e7)] {
                        if i.consume_key(egui::Modifiers::NONE, k) {
                            d += amount;
                        }
                    }
                    d
                });
                if delta != 0.0 {
                    h.scroll_to = Some((h.scroll + delta).max(0.0));
                }
            }
            if let Some(y) = h.scroll_to.take() {
                area = area.vertical_scroll_offset(y);
            }
            let out = area.show(ui, |ui| h.render.show(ui, hl, font_size, &blocks));
            h.scroll = out.state.offset.y;
            let frame = out.inner;
            h.headings = frame.headings;
            clicked = frame.clicked;
            if let Some(anchor) = h.pending_anchor.take() {
                match h.headings.iter().find(|(s, _)| *s == anchor) {
                    Some((_, y)) => {
                        let want = (y - 12.0).max(0.0);
                        if (h.scroll - want).abs() > 0.5 {
                            h.scroll_to = Some(want);
                        }
                        h.anchor_frames += 1;
                        if h.anchor_frames < 6 {
                            h.pending_anchor = Some(anchor);
                        } else {
                            h.anchor_frames = 0;
                        }
                        ui.ctx().request_repaint();
                    }
                    // Not laid out yet: try again next frame.
                    None if h.headings.is_empty() => h.pending_anchor = Some(anchor),
                    None => {}
                }
            }
        });
        if let Some(hit) = open_hit {
            h.go(hit.page, hit.slug);
        }
        if let Some(url) = clicked {
            match help::resolve_link(h.page, &url) {
                Link::Page(p, anchor) if p == h.page => {
                    if let Some(a) = anchor {
                        h.back.push((h.page, h.scroll));
                        h.forward.clear();
                        h.pending_anchor = Some(a);
                        h.anchor_frames = 0;
                    }
                }
                Link::Page(p, anchor) => h.go(&p, anchor),
                Link::External(u) => ui.ctx().open_url(egui::OpenUrl::new_tab(u)),
                Link::Broken(u) => return Some(format!("Help: no page for {u}")),
            }
        }
    }
    None
}

fn search_id() -> egui::Id {
    egui::Id::new("strata-help-search")
}

fn content_id() -> egui::Id {
    egui::Id::new("strata-help-content")
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::Cmd;
    use egui::{Context, Event, Modifiers, Pos2, RawInput, Rect};

    fn frame(app: &mut StrataApp, ctx: &Context, events: Vec<Event>) {
        let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 800.0))), events, ..Default::default() };
        let mut out = ctx.run_ui(raw, |ui| {
            let mut frame = eframe::Frame::_new_kittest();
            eframe::App::ui(app, ui, &mut frame);
        });
        out.textures_delta.clear();
    }

    fn press(key: Key, modifiers: Modifiers) -> Event {
        Event::Key { key, physical_key: Some(key), pressed: true, repeat: false, modifiers }
    }

    #[test]
    fn help_opens_from_commands_and_keys_and_navigates() {
        let mut app = StrataApp::new(None, None);
        let ctx = Context::default();
        frame(&mut app, &ctx, vec![]);
        // F1 opens the manual at the welcome page.
        frame(&mut app, &ctx, vec![press(Key::F1, Modifiers::NONE)]);
        assert!(app.help.open);
        assert_eq!(app.help.page(), help::WELCOME);
        for _ in 0..3 {
            frame(&mut app, &ctx, vec![]);
        }
        assert!(app.help.rect.is_some_and(|r| r.width() > 300.0), "the window was drawn");
        assert!(!app.help.headings.is_empty(), "the page was laid out");
        assert!(!app.docs[0].view.has_focus(&ctx), "the help has the keyboard");

        // The shortcuts page, from its command.
        app.run(&ctx, Cmd::KeyboardShortcuts);
        assert_eq!(app.help.page(), help::SHORTCUTS);
        frame(&mut app, &ctx, vec![]);
        frame(&mut app, &ctx, vec![]);
        assert!(app.help.headings.iter().any(|(s, _)| s == "file"));

        // Back returns to the welcome page; forward to the shortcuts.
        frame(&mut app, &ctx, vec![press(Key::ArrowLeft, Modifiers::ALT)]);
        assert_eq!(app.help.page(), help::WELCOME);
        frame(&mut app, &ctx, vec![press(Key::ArrowRight, Modifiers::ALT)]);
        assert_eq!(app.help.page(), help::SHORTCUTS);

        // A link with an anchor scrolls to its heading.
        app.dev_help("find.md#goto-anything", false);
        for _ in 0..4 {
            frame(&mut app, &ctx, vec![]);
        }
        assert_eq!(app.help.page(), "find.md");
        assert!(app.help.scroll > 50.0, "scrolled to the heading ({})", app.help.scroll);

        // Find focuses the help's search instead of opening the find bar.
        app.run(&ctx, Cmd::Find);
        frame(&mut app, &ctx, vec![]);
        assert!(!app.find.open);
        assert!(ctx.memory(|m| m.has_focus(search_id())));
        frame(&mut app, &ctx, vec![Event::Text("snippet fields".into())]);
        frame(&mut app, &ctx, vec![]);
        assert!(app.help.results);
        assert!(app.help.hits().iter().any(|h| h.page == "snippets.md"));
        // Esc clears the search, then closes the help.
        frame(&mut app, &ctx, vec![press(Key::Escape, Modifiers::NONE)]);
        assert!(app.help.open && app.help.search.is_empty() && !app.help.results);
        frame(&mut app, &ctx, vec![press(Key::Escape, Modifiers::NONE)]);
        assert!(!app.help.open);
        frame(&mut app, &ctx, vec![]);
        assert!(app.docs[0].view.has_focus(&ctx), "the editor has the keyboard back");

        // Documentation reopens at the last page read.
        app.run(&ctx, Cmd::Documentation);
        assert_eq!(app.help.page(), "find.md");
        app.run(&ctx, Cmd::GettingStarted);
        assert_eq!(app.help.page(), help::WELCOME);
        // Close File closes the help, not the tab.
        app.run(&ctx, Cmd::CloseTab);
        assert!(!app.help.open);
        assert_eq!(app.docs.len(), 1);
    }

    #[test]
    fn help_menu_and_palette_entries_exist() {
        let app = StrataApp::new(None, None);
        let menus = app.menus();
        let help = &menus.iter().find(|(n, _)| n == "Help").unwrap().1;
        let labels: Vec<String> = help
            .iter()
            .filter_map(|i| match i {
                crate::menus::Item::Action { label, .. } => Some(label.clone()),
                _ => None,
            })
            .collect();
        for want in ["Documentation", "Keyboard Shortcuts", "Search Documentation…", "Getting Started"] {
            assert!(labels.iter().any(|l| l == want), "{want} in {labels:?}");
        }
        for c in [Cmd::Documentation, Cmd::KeyboardShortcuts, Cmd::SearchDocumentation] {
            assert!(c.name().starts_with("Help: "));
        }
        assert_eq!(Cmd::Documentation.shortcut_label(), Some(crate::keys::label(Modifiers::NONE, Key::F1)));
    }
}
