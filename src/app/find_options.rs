//! The find bar's options beyond the query text, after Sublime's find
//! panel: In Selection (search and replace only inside the selection the
//! option captured), Wrap, Preserve Case and Highlight Matches, plus the
//! regex / case / whole-word toggles as commands with keys. The options
//! are part of each window's session.

use super::{FindBar, StrataApp};
use crate::commands::Cmd;
use crate::find::{FindOptions, Query};
use crate::replace::Replacer;
use std::ops::Range;

impl Default for FindBar {
    fn default() -> Self {
        let o = FindOptions::default();
        Self {
            open: false,
            replace: false,
            query: Query::default(),
            replacement: String::new(),
            focus_query: false,
            origin: 0,
            last_query: Query::default(),
            in_selection: o.in_selection,
            wrap: o.wrap,
            preserve_case: o.preserve_case,
            highlight: o.highlight_matches,
        }
    }
}

/// (command, button text, what it does). Button text uses glyphs every
/// bundled interface font has.
const SEARCH_TOGGLES: &[(Cmd, &str, &str)] = &[
    (Cmd::ToggleFindRegex, ".*", "Regular expression"),
    (Cmd::ToggleFindCase, "Aa", "Case sensitive"),
    (Cmd::ToggleFindWholeWord, "\"ab\"", "Whole word"),
    (Cmd::ToggleFindInSelection, "☰", "In selection: search only the selected text"),
    (Cmd::ToggleFindWrap, "⟳", "Wrap: continue from the other end of the file"),
    (Cmd::ToggleFindHighlight, "▣", "Highlight matches"),
];

fn is_on(f: &FindBar, cmd: Cmd) -> bool {
    match cmd {
        Cmd::ToggleFindRegex => f.query.regex,
        Cmd::ToggleFindCase => f.query.case_sensitive,
        Cmd::ToggleFindWholeWord => f.query.whole_word,
        Cmd::ToggleFindInSelection => f.in_selection,
        Cmd::ToggleFindWrap => f.wrap,
        Cmd::ToggleFindPreserveCase => f.preserve_case,
        Cmd::ToggleFindHighlight => f.highlight,
        _ => false,
    }
}

/// One toggle button; its tooltip names the shortcut in effect.
fn toggle(ui: &mut egui::Ui, f: &FindBar, cmd: Cmd, text: &str, what: &str) -> bool {
    let tip = match cmd.shortcut_label() {
        Some(k) => format!("{what} ({k})"),
        None => what.to_owned(),
    };
    ui.selectable_label(is_on(f, cmd), text).on_hover_text(tip).clicked()
}

/// The toggles left of the Find field: the one clicked, if any.
pub(super) fn search_toggles(ui: &mut egui::Ui, f: &FindBar) -> Option<Cmd> {
    ui.horizontal(|ui| {
        let mut hit = None;
        for &(cmd, text, what) in SEARCH_TOGGLES {
            if toggle(ui, f, cmd, text, what) {
                hit = Some(cmd);
            }
        }
        hit
    })
    .inner
}

/// The toggle left of the Replace field.
pub(super) fn replace_toggles(ui: &mut egui::Ui, f: &FindBar) -> Option<Cmd> {
    ui.horizontal(|ui| {
        let what = "Preserve case: \"foo\" → \"bar\" makes \"Foo\" \"Bar\" and \"FOO\" \"BAR\"";
        toggle(ui, f, Cmd::ToggleFindPreserveCase, "AB", what).then_some(Cmd::ToggleFindPreserveCase)
    })
    .inner
}

impl StrataApp {
    /// A find option command (a toggle button or its key).
    pub(super) fn toggle_find_option(&mut self, cmd: Cmd) {
        let f = &mut self.find;
        let (name, flag) = match cmd {
            Cmd::ToggleFindRegex => ("Regular expression", &mut f.query.regex),
            Cmd::ToggleFindCase => ("Case sensitive", &mut f.query.case_sensitive),
            Cmd::ToggleFindWholeWord => ("Whole word", &mut f.query.whole_word),
            Cmd::ToggleFindInSelection => ("In selection", &mut f.in_selection),
            Cmd::ToggleFindWrap => ("Wrap", &mut f.wrap),
            Cmd::ToggleFindPreserveCase => ("Preserve case", &mut f.preserve_case),
            Cmd::ToggleFindHighlight => ("Highlight matches", &mut f.highlight),
            _ => return,
        };
        *flag = !*flag;
        let on = *flag;
        if cmd == Cmd::ToggleFindInSelection && on {
            self.capture_find_scope();
        }
        self.status = format!("Find: {name} {}", if on { "on" } else { "off" });
    }

    /// The selections (cursors excluded) become In Selection's scope.
    pub(super) fn capture_find_scope(&mut self) {
        let ed = &mut self.docs[self.active].editor;
        let ranges = ed.selections().iter().filter(|s| !s.is_empty()).map(|s| s.range()).collect();
        ed.set_find_scope(Some(ranges));
    }

    /// The bar is opening. Like Sublime, a selection spanning lines turns In
    /// Selection on when `auto_find_in_selection` is set; with In Selection
    /// on, a selection spanning lines (or several selections) is the new scope.
    pub(super) fn find_opening(&mut self) {
        let ed = &self.docs[self.active].editor;
        let b = &ed.buffer;
        let selected: Vec<_> = ed.selections().iter().filter(|s| !s.is_empty()).collect();
        let multi_line = selected.iter().any(|s| b.line_of(s.start()) != b.line_of(s.end()));
        if multi_line && self.settings.auto_find_in_selection {
            self.find.in_selection = true;
        }
        if self.find.in_selection && (multi_line || selected.len() > 1) {
            self.capture_find_scope();
        }
    }

    /// The char ranges searched: None for the whole file. A tab that has no
    /// scope yet takes its current selection.
    pub(super) fn find_scope(&mut self) -> Option<Vec<Range<usize>>> {
        if !self.find.in_selection {
            return None;
        }
        if self.docs[self.active].editor.find_scope().is_none() {
            self.capture_find_scope();
        }
        self.docs[self.active].editor.find_scope().map(<[_]>::to_vec)
    }

    /// One match's replacement, in the matched text's case with Preserve Case.
    pub(super) fn replacement_for(&self, r: &Replacer, caps: &regex::Captures, eol: &str) -> String {
        let rep = r.expand(caps, eol);
        match caps.get(0) {
            Some(m) if self.find.preserve_case => crate::find::preserve_case(m.as_str(), &rep),
            _ => rep,
        }
    }

    /// Replace All's edits (char ranges, sorted): every match in the file,
    /// or with In Selection only those inside the scope.
    pub(super) fn replace_all_edits(&mut self, r: &Replacer) -> Vec<(Range<usize>, String)> {
        let scope = self.find_scope();
        let ed = &mut self.docs[self.active].editor;
        let text = ed.text_snapshot();
        let eol = ed.buffer.line_ending().as_str();
        let ranges = match &scope {
            Some(s) => crate::find::scope_bytes(ed, s),
            None => vec![0..text.len()],
        };
        let mut edits = Vec::new();
        let (mut byte, mut chr) = (0, 0);
        for range in ranges {
            let hay = &text[..range.end];
            let mut at = range.start;
            // Where the last replaced match ended.
            let mut last_end = None;
            while at <= hay.len() {
                let Some(caps) = r.regex().captures_at(hay, at) else { break };
                let m = caps.get(0).expect("group 0");
                // After an empty match, move on a char (like `captures_iter`).
                at = if m.is_empty() { m.end() + hay[m.end()..].chars().next().map_or(1, char::len_utf8) } else { m.end() };
                // Like `Regex::replace_all`: no empty match right where the
                // previous match ended ("a*" → "X" in "baaac" is "XbXcX").
                if m.is_empty() && last_end == Some(m.start()) {
                    continue;
                }
                // Never replace half of a `\r\n` (see `find::replaceable`).
                let Some(span) = crate::find::replaceable(&text, &caps) else { continue };
                if span.start < byte || span.end > range.end {
                    continue;
                }
                chr += text[byte..span.start].chars().count();
                let start = chr;
                chr += text[span.clone()].chars().count();
                byte = span.end;
                at = at.max(span.end);
                last_end = Some(span.end);
                edits.push((start..chr, self.replacement_for(r, &caps, eol)));
            }
        }
        edits
    }

    /// Dev aid (pair with STRATA_SCREENSHOT and STRATA_DEV_FIND):
    /// STRATA_DEV_SELECT="4..20,30..41" selects those char ranges first.
    pub(super) fn dev_select(&mut self) {
        let Some(spec) = crate::dev::var("STRATA_DEV_SELECT") else { return };
        let ranges: Vec<Range<usize>> = spec
            .split(',')
            .filter_map(|r| r.split_once(".."))
            .filter_map(|(a, b)| Some(a.trim().parse().ok()?..b.trim().parse().ok()?))
            .collect();
        let len = self.docs[self.active].editor.buffer.len_chars();
        let ranges: Vec<_> = ranges.into_iter().map(|r| r.start.min(len)..r.end.min(len)).collect();
        self.docs[self.active].editor.select_ranges(&ranges, 0);
    }

    /// The session's copy of the find bar.
    pub(super) fn find_options(&self) -> FindOptions {
        let f = &self.find;
        FindOptions {
            regex: f.query.regex,
            case_sensitive: f.query.case_sensitive,
            whole_word: f.query.whole_word,
            in_selection: f.in_selection,
            wrap: f.wrap,
            preserve_case: f.preserve_case,
            highlight_matches: f.highlight,
            text: f.query.text.clone(),
            replacement: f.replacement.clone(),
        }
    }

    pub(super) fn set_find_options(&mut self, o: &FindOptions) {
        let f = &mut self.find;
        f.query = Query { text: o.text.clone(), regex: o.regex, case_sensitive: o.case_sensitive, whole_word: o.whole_word };
        f.last_query = f.query.clone();
        f.replacement.clone_from(&o.replacement);
        f.in_selection = o.in_selection;
        f.wrap = o.wrap;
        f.preserve_case = o.preserve_case;
        f.highlight = o.highlight_matches;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;
    use crate::editor::Editor;

    fn app_with(text: &str) -> StrataApp {
        let mut app = StrataApp::new(None, None);
        app.docs[0].editor = Editor::new(Buffer::from_str(text));
        app
    }

    fn text(app: &StrataApp) -> String {
        let e = &app.docs[app.active].editor;
        e.buffer.slice(0..e.buffer.len_chars())
    }

    fn search(app: &mut StrataApp, text: &str, replacement: &str) {
        app.find.query = Query { text: text.into(), regex: false, case_sensitive: true, whole_word: false };
        app.find.replacement = replacement.into();
    }

    fn sel(app: &StrataApp) -> Range<usize> {
        app.docs[app.active].editor.primary().range()
    }

    #[test]
    fn defaults_match_sublime() {
        let f = FindBar::default();
        assert!(f.wrap && f.highlight, "wrap and highlight start on");
        assert!(!f.in_selection && !f.preserve_case);
        assert_eq!(StrataApp::new(None, None).find_options(), FindOptions::default());
    }

    #[test]
    fn in_selection_limits_find_and_find_all() {
        // Lines: "x a" "x b" "x c" "x d".
        let mut app = app_with("x a\nx b\nx c\nx d\n");
        app.doc().editor.select_ranges(&[4..11], 0);
        search(&mut app, "x", "");
        app.toggle_find_option(Cmd::ToggleFindInSelection);
        app.doc().editor.select_ranges(&[0..0], 0);
        app.find_step(true);
        assert_eq!(sel(&app), 4..5, "the first match inside the selection");
        app.find_step(true);
        assert_eq!(sel(&app), 8..9);
        app.find_step(true);
        assert_eq!(sel(&app), 4..5, "wraps within the selection");
        app.find_all();
        let ranges: Vec<_> = app.docs[0].editor.selections().iter().map(|s| s.range()).collect();
        assert_eq!(ranges, vec![4..5, 8..9]);
    }

    #[test]
    fn in_selection_with_several_selections_and_replace_all_is_one_undo_step() {
        let mut app = app_with("foo foo\nfoo foo\nfoo foo\n");
        // Multi-cursor: the second word of line 1 and all of line 3.
        app.doc().editor.select_ranges(&[4..7, 16..23], 0);
        search(&mut app, "foo", "quux");
        app.toggle_find_option(Cmd::ToggleFindInSelection);
        app.replace_all();
        assert_eq!(text(&app), "foo quux\nfoo foo\nquux quux\n");
        assert_eq!(app.status, "Replaced 3 occurrences in the selection");
        // The scope followed the edits: it still covers exactly the replaced text.
        let scope = app.docs[0].editor.find_scope().unwrap().to_vec();
        assert_eq!(scope, vec![4..8, 17..26]);
        let t = text(&app);
        assert_eq!(&t[4..8], "quux");
        assert_eq!(&t[17..26], "quux quux");
        app.doc().editor.undo();
        assert_eq!(text(&app), "foo foo\nfoo foo\nfoo foo\n", "one undo step");
    }

    #[test]
    fn the_scope_tracks_edits_made_before_replacing() {
        let mut app = app_with("aa\nbb aa\naa\n");
        app.doc().editor.select_ranges(&[3..8], 0);
        search(&mut app, "aa", "c");
        app.toggle_find_option(Cmd::ToggleFindInSelection);
        // Typing before the scope shifts it.
        app.doc().editor.select_ranges(&[0..0], 0);
        app.doc().editor.insert_text("zz ");
        assert_eq!(app.docs[0].editor.find_scope().unwrap(), &[6..11]);
        app.replace_all();
        assert_eq!(text(&app), "zz aa\nbb c\naa\n");
    }

    #[test]
    fn replace_one_stays_in_the_selection_and_preserves_case() {
        let mut app = app_with("Foo foo FOO\nFoo\n");
        app.doc().editor.select_ranges(&[0..11], 0);
        search(&mut app, "foo", "bar");
        app.find.query.case_sensitive = false;
        app.toggle_find_option(Cmd::ToggleFindInSelection);
        app.toggle_find_option(Cmd::ToggleFindPreserveCase);
        app.doc().editor.select_ranges(&[0..0], 0);
        app.find_step(true);
        for _ in 0..4 {
            app.replace_one();
        }
        assert_eq!(text(&app), "Bar bar BAR\nFoo\n", "the last line is outside the selection");
    }

    #[test]
    fn preserve_case_in_replace_all_and_off_by_default() {
        let mut app = app_with("foo Foo FOO");
        search(&mut app, "foo", "bar");
        app.find.query.case_sensitive = false;
        app.replace_all();
        assert_eq!(text(&app), "bar bar bar");
        let mut app = app_with("foo Foo FOO");
        search(&mut app, "foo", "bar");
        app.find.query.case_sensitive = false;
        app.find.preserve_case = true;
        app.replace_all();
        assert_eq!(text(&app), "bar Bar BAR");
    }

    #[test]
    fn regex_groups_still_expand_with_preserve_case() {
        let mut app = app_with("Hello world");
        app.find.query = Query { text: r"(\w+) (\w+)".into(), regex: true, ..Default::default() };
        app.find.replacement = r"\2 \1".into();
        app.find.preserve_case = true;
        app.replace_all();
        assert_eq!(text(&app), "World Hello", "the whole match is capitalized");
    }

    #[test]
    fn without_wrap_find_next_stops_at_the_end() {
        let mut app = app_with("cat dog cat dog");
        search(&mut app, "dog", "");
        app.toggle_find_option(Cmd::ToggleFindWrap);
        assert!(!app.find.wrap);
        app.find_step(true);
        app.find_step(true);
        assert_eq!(sel(&app), 12..15);
        app.find_step(true);
        assert_eq!(sel(&app), 12..15, "stays on the last match");
        assert!(app.status.contains("Wrap is off"), "{}", app.status);
        app.find_step(false);
        app.find_step(false);
        assert_eq!(sel(&app), 4..7);
    }

    #[test]
    fn auto_find_in_selection_turns_it_on_for_multi_line_selections() {
        let mut app = app_with("ab\nab\nab\n");
        app.doc().editor.select_ranges(&[0..4], 0);
        app.open_find(false);
        assert!(!app.find.in_selection, "off by default, like Sublime");
        app.settings.auto_find_in_selection = true;
        app.doc().editor.select_ranges(&[0..2], 0);
        app.open_find(false);
        assert!(!app.find.in_selection, "a one-line selection is the search text instead");
        assert_eq!(app.find.query.text, "ab");
        app.doc().editor.select_ranges(&[3..9], 0);
        app.open_find(false);
        assert!(app.find.in_selection);
        assert_eq!(app.docs[0].editor.find_scope().unwrap(), &[3..9]);
        app.refresh_matches();
        assert_eq!(app.docs[0].finder.matches, vec![3..5, 6..8]);
    }

    #[test]
    fn an_empty_selection_says_so() {
        let mut app = app_with("ab ab");
        search(&mut app, "ab", "");
        app.toggle_find_option(Cmd::ToggleFindInSelection);
        app.refresh_matches();
        assert_eq!(app.match_label().text(), "No selection");
    }

    #[test]
    fn highlight_matches_off_hides_the_outlines() {
        let mut app = app_with("x");
        app.toggle_find_option(Cmd::ToggleFindHighlight);
        assert!(!app.find.highlight);
        assert_eq!(app.status, "Find: Highlight matches off");
    }

    #[test]
    fn options_round_trip_through_the_session() {
        let mut app = app_with("x");
        search(&mut app, "needle", "pin");
        for cmd in [Cmd::ToggleFindRegex, Cmd::ToggleFindInSelection, Cmd::ToggleFindWrap, Cmd::ToggleFindPreserveCase] {
            app.toggle_find_option(cmd);
        }
        let saved = app.find_options();
        let json = serde_json::to_string(&saved).unwrap();
        let mut back = StrataApp::new(None, None);
        back.set_find_options(&serde_json::from_str(&json).unwrap());
        assert_eq!(back.find_options(), saved);
        assert!(back.find.query.regex && back.find.in_selection && !back.find.wrap && back.find.preserve_case);
        // An older session without the field gets the defaults.
        let old: crate::session::WindowState = serde_json::from_str("{}").unwrap();
        assert_eq!(old.find, FindOptions::default());
    }

    #[test]
    fn toggle_labels_have_glyphs_in_the_interface_font() {
        let ctx = egui::Context::default();
        ctx.run_ui(Default::default(), |_| {}).textures_delta.clear();
        let font = egui::FontId::proportional(13.0);
        for (_, text, _) in SEARCH_TOGGLES {
            assert!(ctx.fonts_mut(|f| f.has_glyphs(&font, text)), "{text}");
        }
    }

    #[test]
    fn find_bar_shows_every_toggle_with_its_key_in_the_tooltip() {
        let mut app = app_with("abc");
        app.open_find(true);
        let ctx = egui::Context::default();
        let raw = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 200.0))), ..Default::default() };
        let mut out = ctx.run_ui(raw, |ui| app.find_bar(ui));
        out.textures_delta.clear();
        let texts: Vec<String> = out
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Text(t) => Some(t.galley.job.text.clone()),
                _ => None,
            })
            .collect();
        for label in SEARCH_TOGGLES.iter().map(|t| t.1).chain(["AB"]) {
            assert!(texts.iter().any(|t| t == label), "{label} missing from {texts:?}");
        }
        let tip = |cmd: Cmd| format!("({})", cmd.shortcut_label().unwrap());
        assert!(tip(Cmd::ToggleFindInSelection).contains('L'));
    }

    fn regex_replace_all(text: &str, pattern: &str, replacement: &str) -> String {
        let mut app = app_with(text);
        app.find.query = Query { text: pattern.into(), regex: true, case_sensitive: true, whole_word: false };
        app.find.replacement = replacement.into();
        app.replace_all();
        self::text(&app)
    }

    #[test]
    fn regex_replace_all_keeps_crlf_line_breaks_whole() {
        assert_eq!(regex_replace_all("a  \r\nb  \r\nc", r"\s+$", ""), "a\r\nb\r\nc");
        assert_eq!(regex_replace_all("ab\r\ncd", "^(.*)$", "<$1>"), "<ab>\r\n<cd>");
        assert_eq!(regex_replace_all("foo\r\nfoo bar\r\n", "foo$", "X"), "X\r\nfoo bar\r\n");
        assert_eq!(regex_replace_all("a\r\nb", r"a\r\nb", "c"), "c", "whole line breaks still match");
        // `\n` (or `\s`) takes the whole `\r\n`, never half of it.
        assert_eq!(regex_replace_all("a\r\nb\r\nc", r"\n", " "), "a b c");
        assert_eq!(regex_replace_all("a.\r\nb", r"\.\s", "!"), "a!b");
        assert_eq!(regex_replace_all("x\r\ny", r"[^x]y", "Z"), "xZ");
        // A group holding half a line break would write a lone `\r`: skipped.
        assert_eq!(regex_replace_all("x\r\ny", r"x(\s)", "[$1]"), "x\r\ny");
        // A CRLF file gets exactly what the same text with LF breaks gets.
        let cases = [
            ("a  \nb\n", r"\s+$", ""),
            ("a  \n\n b\n", r"\s+", "_"),
            ("ab\ncd\n", "^(.*)$", "<$1>"),
            ("one\ntwo\n", r"(\w+)\n", "$1;"),
            ("k = 1\nk = 2\n", r"^k = (.)$", "$1"),
            ("x.\ny\n", r".$", "!"),
        ];
        for (lf, pattern, rep) in cases {
            let want = regex_replace_all(lf, pattern, rep).replace('\n', "\r\n");
            assert_eq!(regex_replace_all(&lf.replace('\n', "\r\n"), pattern, rep), want, "{pattern:?} on {lf:?}");
        }
    }

    #[test]
    fn find_does_not_match_inside_crlf() {
        let mut app = app_with("foo  \r\nbar\r\nx");
        app.find.query = Query { text: r"\s+$".into(), regex: true, case_sensitive: true, whole_word: false };
        app.refresh_matches();
        assert_eq!(app.docs[0].finder.matches, vec![3..5]);
        app.find.query.text = r"\n".into();
        app.refresh_matches();
        assert_eq!(app.docs[0].finder.matches, vec![5..7, 10..12], "whole line breaks");
        // Replace (one) replaces the selected, grown match.
        app.find.replacement = " ".into();
        app.doc().editor.select_ranges(&[5..7], 0);
        app.replace_one();
        assert_eq!(text(&app), "foo   bar\r\nx");
    }

    #[test]
    fn replace_all_with_empty_matches_follows_regex_semantics() {
        // No empty match right after a real one (as `Regex::replace_all`).
        assert_eq!(regex_replace_all("baaac", "a*", "X"), "XbXcX");
        assert_eq!(regex_replace_all("baaac", "a*", "X"), regex::Regex::new("a*").unwrap().replace_all("baaac", "X"));
    }

    #[test]
    fn toggle_keys_reach_the_find_bar() {
        let mut app = app_with("x");
        let ctx = egui::Context::default();
        app.run(&ctx, Cmd::ToggleFindCase);
        assert!(app.find.query.case_sensitive);
        app.run(&ctx, Cmd::ToggleFindWholeWord);
        assert!(app.find.query.whole_word);
        // Sublime's keys: ⌥⌘L / Alt+L for In Selection.
        let l = if crate::keys::MAC { (crate::keys::CMD_ALT, egui::Key::L) } else { (crate::keys::ALT, egui::Key::L) };
        assert_eq!(Cmd::ToggleFindInSelection.keys(), vec![l]);
    }
}
