//! The menu bar's contents (Sublime's layout: File, Edit, Selection, Find,
//! View, Goto, Tools, Project, Window, Help) and the editor-command table
//! shared by menus, palette and shortcuts.

use super::StrataApp;
use crate::commands::Cmd;
use crate::keys::*;
use crate::keybindings::Target;
use crate::menus::{Act, Item, Menus, System};
use egui::{Key, Modifiers};

/// An editor command: id (for `run_editor`), palette name, and default
/// shortcuts: single keys and/or a two-key chord. The keys in effect come from
/// the keymap (see `keybindings`), which starts from these.
pub struct EdCmd {
    pub id: &'static str,
    pub name: &'static str,
    pub keys: &'static [(Modifiers, Key)],
    pub chord: Option<[(Modifiers, Key); 2]>,
}

const fn ed(id: &'static str, name: &'static str, keys: &'static [(Modifiers, Key)]) -> EdCmd {
    EdCmd { id, name, keys, chord: None }
}

const fn chord(id: &'static str, name: &'static str, first: (Modifiers, Key), then: (Modifiers, Key)) -> EdCmd {
    EdCmd { id, name, keys: &[], chord: Some([first, then]) }
}

pub const EDITOR_COMMANDS: &[EdCmd] = &[
    ed("toggle_comment", "Edit: Toggle Comment", &[(CMD, Key::Slash)]),
    ed("indent", "Edit: Indent", &[(CMD, Key::CloseBracket)]),
    ed("outdent", "Edit: Unindent", &[(CMD, Key::OpenBracket)]),
    ed("duplicate", "Edit: Duplicate Line", &[(CMD_SHIFT, Key::D)]),
    ed("delete_line", "Edit: Delete Line", &[(CTRL_SHIFT, Key::K)]),
    ed("move_up", "Edit: Swap Line Up", if MAC { &[(CTRL_CMD, Key::ArrowUp)] } else { &[(CTRL_SHIFT, Key::ArrowUp)] }),
    ed("move_down", "Edit: Swap Line Down", if MAC { &[(CTRL_CMD, Key::ArrowDown)] } else { &[(CTRL_SHIFT, Key::ArrowDown)] }),
    ed("join", "Edit: Join Lines", &[(CMD, Key::J)]),
    ed("line_after", "Edit: Insert Line After", &[(CMD, Key::Enter)]),
    ed("line_before", "Edit: Insert Line Before", &[(CMD_SHIFT, Key::Enter)]),
    chord("upper", "Edit: Convert Case: Upper Case", (CMD, Key::K), (CMD, Key::U)),
    chord("lower", "Edit: Convert Case: Lower Case", (CMD, Key::K), (CMD, Key::L)),
    ed("sort", "Edit: Sort Lines", if MAC { &[(NONE, Key::F5)] } else { &[(NONE, Key::F9)] }),
    ed("sort_case", "Edit: Sort Lines (Case Sensitive)", if MAC { &[(CTRL, Key::F5)] } else { &[(CTRL, Key::F9)] }),
    ed("reverse", "Edit: Permute Lines: Reverse", &[]),
    ed("unique", "Edit: Permute Lines: Unique", &[]),
    ed("trim", "Edit: Trim Trailing Whitespace", &[]),
    ed("expand_line", "Selection: Expand Selection to Line", &[(CMD, Key::L)]),
    ed("split_lines", "Selection: Split into Lines", &[(CMD_SHIFT, Key::L)]),
    ed("cursor_above", "Selection: Add Previous Line", if MAC { &[(CTRL_SHIFT, Key::ArrowUp)] } else { &[(CTRL_ALT, Key::ArrowUp)] }),
    ed("cursor_below", "Selection: Add Next Line", if MAC { &[(CTRL_SHIFT, Key::ArrowDown)] } else { &[(CTRL_ALT, Key::ArrowDown)] }),
    ed("next_occurrence", "Find: Quick Add Next", &[(CMD, Key::D)]),
    ed("all_occurrences", "Find: Quick Find All", if MAC { &[(CTRL_CMD, Key::G)] } else { &[(ALT, Key::F3)] }),
    ed("expand_brackets", "Selection: Expand Selection to Brackets", &[(CTRL_SHIFT, Key::M)]),
    ed("jump_bracket", "Goto: Jump to Matching Bracket", &[(CTRL, Key::M)]),
    ed("single_selection", "Selection: Single Selection", &[]),
    ed("toggle_block_comment", "Edit: Comment: Toggle Block Comment", if MAC { &[(CMD_ALT, Key::Slash)] } else { &[(CTRL_SHIFT, Key::Slash)] }),
    ed("lower_camel", "Edit: Convert Case: lowerCamelCase", &[]),
    ed("upper_camel", "Edit: Convert Case: UpperCamelCase", &[]),
    ed("snake", "Edit: Convert Case: snake_case", &[]),
    ed("kebab", "Edit: Convert Case: kebab-case", &[]),
    ed("sel_sort_ci", "Edit: Permute Selections: Sort", &[]),
    ed("expand_selection", "Selection: Expand Selection", &[]),
    ed("expand_line_up", "Selection: Expand Selection to Line Upward", &[]),
    ed("expand_block", "Selection: Expand Selection to Block", &[]),
    ed("set_mark", "Edit: Mark: Set Mark", &[]),
    ed("select_to_mark", "Edit: Mark: Select to Mark", &[]),
    ed("delete_to_mark", "Edit: Mark: Delete to Mark", &[]),
    ed("swap_mark", "Edit: Mark: Swap with Mark", &[]),
    ed("clear_mark", "Edit: Mark: Clear Mark", &[]),
    ed("yank", "Edit: Mark: Yank", &[]),
    ed("soft_undo", "Edit: Undo Selection: Soft Undo", &[(CMD, Key::U)]),
    ed("soft_redo", "Edit: Undo Selection: Soft Redo", &[(CMD_SHIFT, Key::U)]),
    ed("tx_json_format", "Tools: Text: Format JSON", &[]),
    ed("tx_json_minify", "Tools: Text: Minify JSON", &[]),
    ed("tx_json_sort", "Tools: Text: Sort JSON Keys", &[]),
    ed("tx_b64_encode", "Tools: Text: Base64 Encode", &[]),
    ed("tx_b64_decode", "Tools: Text: Base64 Decode", &[]),
    ed("tx_url_encode", "Tools: Text: URL Encode", &[]),
    ed("tx_url_decode", "Tools: Text: URL Decode", &[]),
    ed("tx_html_escape", "Tools: Text: HTML Escape", &[]),
    ed("tx_html_unescape", "Tools: Text: HTML Unescape", &[]),
    ed("tx_uuid", "Tools: Text: Insert UUID", &[]),
    ed("tx_date", "Tools: Text: Insert Date and Time", &[]),
    ed("delete_word_forward", "Edit: Text: Delete Word Forward", &[]),
    ed("delete_word_backward", "Edit: Text: Delete Word Backward", &[]),
    ed("paste_and_indent", "Edit: Paste and Indent", &[(CMD_SHIFT, Key::V)]),
    ed("delete_to_end", "Edit: Text: Delete to End", if MAC { &[(CTRL, Key::K)] } else { &[] }),
    ed("delete_to_start", "Edit: Text: Delete to Beginning", if MAC { &[(CMD, Key::Backspace)] } else { &[] }),
    ed("transpose", "Edit: Text: Transpose", if MAC { &[(CTRL, Key::T)] } else { &[] }),
    ed("title_case", "Edit: Convert Case: Title Case", &[]),
    ed("swap_case", "Edit: Convert Case: Swap Case", &[]),
    ed("shuffle", "Edit: Permute Lines: Shuffle", &[]),
    ed("sel_sort", "Edit: Permute Selections: Sort", &[]),
    ed("sel_reverse", "Edit: Permute Selections: Reverse", &[]),
    ed("sel_unique", "Edit: Permute Selections: Unique", &[]),
    ed("sel_shuffle", "Edit: Permute Selections: Shuffle", &[]),
    ed("invert_selection", "Selection: Invert Selection", &[]),
    ed("expand_word", "Selection: Expand Selection to Word", &[]),
    ed("expand_paragraph", "Selection: Expand Selection to Paragraph", &[]),
    ed("expand_indentation", "Selection: Expand Selection to Indentation", &[(CMD_SHIFT, Key::J)]),
    ed("wrap_paragraph", "Edit: Wrap Paragraph at Ruler", if MAC { &[(CMD_ALT, Key::Q)] } else { &[(ALT, Key::Q)] }),
    ed("reindent", "Edit: Line: Reindent", &[]),
    ed("to_spaces", "View: Indentation: Convert Indentation to Spaces", &[]),
    ed("to_tabs", "View: Indentation: Convert Indentation to Tabs", &[]),
    ed("guess_indent", "View: Indentation: Guess Settings From Buffer", &[]),
    ed("bookmark_toggle", "Goto: Bookmarks: Toggle Bookmark", &[(CMD, Key::F2)]),
    ed("bookmark_next", "Goto: Bookmarks: Next Bookmark", &[(NONE, Key::F2)]),
    ed("bookmark_prev", "Goto: Bookmarks: Previous Bookmark", &[(SHIFT, Key::F2)]),
    ed("bookmark_clear", "Goto: Bookmarks: Clear Bookmarks", &[(CMD_SHIFT, Key::F2)]),
    ed("bookmark_select", "Goto: Bookmarks: Select All Bookmarks", &[(ALT, Key::F2)]),
    ed("select_all", "Selection: Select All", &[(CMD, Key::A)]),
    ed("undo", "Edit: Undo", &[(CMD, Key::Z)]),
    ed("redo", "Edit: Redo", if MAC { &[(CMD_SHIFT, Key::Z)] } else { &[(CMD, Key::Y), (CMD_SHIFT, Key::Z)] }),
];

fn find(id: &str) -> &'static EdCmd {
    EDITOR_COMMANDS.iter().find(|c| c.id == id).unwrap_or_else(|| panic!("unknown editor command {id}"))
}

/// (id, name) for the command palette; it looks up shortcuts in the keymap.
pub fn palette_editor_commands() -> Vec<(&'static str, &'static str)> {
    EDITOR_COMMANDS.iter().map(|c| (c.id, c.name)).collect()
}

/// A menu item for an editor command, labeled `label`.
fn e(label: &str, id: &'static str) -> Item {
    let c = find(id);
    Item::new(label, Act::Editor(c.id)).bound(Target::Editor(c.id))
}

/// A menu item for a folding or scrolling command (see `keybindings`).
fn v(label: &str, target: Target, act: &str) -> Item {
    Item::new(label, Act::Named(act.into())).bound(target)
}

fn c(label: &str, cmd: Cmd) -> Item {
    Item::cmd(label, cmd)
}

fn sub(label: &str, items: Vec<Item>) -> Item {
    Item::Sub(label.into(), items)
}

const SEP: fn() -> Item = || Item::Separator;

/// Settings, color scheme and fonts: under the app menu on macOS, the
/// Preferences menu elsewhere.
fn settings_owned(app: &StrataApp) -> Vec<Item> {
    let s = &app.settings;
    let face = |name: &str| if name.is_empty() { "Default".to_owned() } else { name.to_owned() };
    let mut ui_sizes: Vec<Item> = (10..=18).map(|n| Item::new(format!("{n} pt"), Act::Named(format!("uisize:{n}"))).checked(s.ui_font_size == n as f32)).collect();
    ui_sizes.extend([SEP(), c("Larger", Cmd::UiLarger), c("Smaller", Cmd::UiSmaller)]);
    let plain_spacing = s.line_padding_top == 0.0 && s.line_padding_bottom == 0.0;
    let spacing: Vec<Item> = [("Compact", 1.0), ("Comfortable", 1.2), ("Relaxed", 1.4), ("Spacious", 1.6)]
        .into_iter()
        .map(|(label, h)| Item::new(format!("{label} ({h:.1}×)"), Act::Named(format!("linespacing:{h}"))).checked(plain_spacing && (s.line_height - h).abs() < 0.01))
        .collect();
    vec![
        c("Settings", Cmd::Settings),
        c("Key Bindings", Cmd::KeyBindings),
        SEP(),
        sub("Color Scheme", app.theme_menu_items()),
        SEP(),
        sub(
            "Font",
            vec![
                Item::new(format!("Editor Font: {}…", face(&s.font_face)), Act::Cmd(Cmd::SelectEditorFont)),
                c("Larger", Cmd::ZoomIn),
                c("Smaller", Cmd::ZoomOut),
                c("Reset Size", Cmd::ZoomReset),
                sub("Line Spacing", spacing),
                SEP(),
                Item::new(format!("Interface Font: {}…", face(&s.ui_font_face)), Act::Cmd(Cmd::SelectUiFont)),
                sub("Interface Size", ui_sizes),
            ],
        ),
    ]
}

impl StrataApp {
    /// The whole menu bar for the current state (checks, recent files, tabs).
    pub(super) fn menus(&self) -> Menus {
        let d = &self.docs[self.active];
        let groups = self.layout.groups();
        let (cols, rows) = (self.layout.cols.len(), self.layout.rows.len());
        let mut m: Menus = Vec::new();

        let settings = settings_owned(self);
        if MAC {
            m.push((
                "Strata".into(),
                vec![
                    Item::new("About Strata", Act::Cmd(Cmd::About)),
                    SEP(),
                    sub("Settings", settings),
                    Item::new("Install Shell Command…", Act::Cmd(Cmd::InstallShellCommand)),
                    SEP(),
                    Item::System(System::Services),
                    SEP(),
                    Item::System(System::Hide),
                    Item::System(System::HideOthers),
                    Item::System(System::ShowAll),
                    SEP(),
                    Item::new("Quit Strata", Act::Quit).fixed(CMD, Key::Q),
                    // Hot exit always keeps windows (tabs and unsaved text) for next time.
                    Item::new("Quit and Keep Windows", Act::Quit).fixed(CMD_ALT, Key::Q),
                ],
            ));
        }

        // ---- File
        let mut recent: Vec<Item> = vec![c("Reopen Closed File", Cmd::ReopenClosed).enabled(!self.closed.is_empty()), SEP()];
        recent.extend(self.recent_files.iter().map(|p| Item::new(p.display().to_string(), Act::OpenRecent(p.clone()))));
        if !self.recent_folders.is_empty() {
            recent.push(SEP());
            recent.extend(self.recent_folders.iter().map(|p| Item::new(format!("{}/", p.display()), Act::OpenRecentFolder(p.clone()))));
        }
        recent.extend([SEP(), c("Clear Items", Cmd::ClearRecent)]);
        let encodings = |reopen: bool| -> Item {
            let items = crate::encoding::Encoding::all()
                .iter()
                .map(|e| Item::new(e.name(), if reopen { Act::ReopenWithEncoding(*e) } else { Act::SaveWithEncoding(*e) }).checked(d.encoding == *e))
                .collect();
            sub(if reopen { "Reopen with Encoding" } else { "Save with Encoding" }, items)
        };
        let mut file = vec![
            c("New File", Cmd::NewTab),
            c("New Window", Cmd::NewWindow),
            c("Open…", Cmd::Open),
            c("Open Folder…", Cmd::OpenFolder),
            sub("Open Recent", recent),
            encodings(true),
            c("Save", Cmd::Save),
            encodings(false),
            c("Save As…", Cmd::SaveAs),
            c("Save All", Cmd::SaveAll),
            SEP(),
            sub("Export", vec![c("Markdown as HTML…", Cmd::ExportMarkdownHtml).enabled(crate::markdown::is_markdown(d.path.as_deref(), d.highlighter.syntax_name()))]),
            SEP(),
            c("Close Window", Cmd::CloseWindow),
            SEP(),
            c("Close File", Cmd::CloseTab),
            c("Revert File", Cmd::RevertFile).enabled(d.path.is_some()),
            c("Close All Files", Cmd::CloseAll),
        ];
        if !MAC {
            file.extend([SEP(), Item::new("Exit", Act::Quit)]);
        }
        m.push(("File".into(), file));

        // ---- Edit
        let wraps = [70, 72, 78, 80, 100, 120];
        let mut wrap = vec![e("Wrap Paragraph at Ruler", "wrap_paragraph")];
        wrap.extend(wraps.iter().map(|w| Item::new(format!("Wrap paragraph at {w} characters"), Act::Named(format!("wrap:{w}")))));
        m.push((
            "Edit".into(),
            vec![
                Item::new("Undo", Act::Undo).bound(Target::Editor("undo")),
                Item::new("Redo", Act::Redo).bound(Target::Editor("redo")),
                sub("Undo Selection", vec![e("Soft Undo", "soft_undo"), e("Soft Redo", "soft_redo")]),
                SEP(),
                Item::new("Cut", Act::Cut).fixed(CMD, Key::X),
                Item::new("Copy", Act::Copy).fixed(CMD, Key::C),
                c("Copy as HTML", Cmd::CopyAsHtml),
                Item::new("Paste", Act::Paste).fixed(CMD, Key::V),
                e("Paste and Indent", "paste_and_indent"),
                c("Paste from History", Cmd::PasteFromHistory),
                SEP(),
                sub(
                    "Line",
                    vec![
                        e("Indent", "indent"),
                        e("Unindent", "outdent"),
                        e("Reindent", "reindent"),
                        e("Swap Line Up", "move_up"),
                        e("Swap Line Down", "move_down"),
                        e("Duplicate Line", "duplicate"),
                        e("Delete Line", "delete_line"),
                        e("Join Lines", "join"),
                    ],
                ),
                sub("Comment", vec![e("Toggle Comment", "toggle_comment"), e("Toggle Block Comment", "toggle_block_comment")]),
                sub(
                    "Text",
                    vec![
                        e("Insert Line Before", "line_before"),
                        e("Insert Line After", "line_after"),
                        SEP(),
                        e("Delete Word Forward", "delete_word_forward"),
                        e("Delete Word Backward", "delete_word_backward"),
                        e("Delete Line", "delete_line"),
                        e("Delete to End", "delete_to_end"),
                        e("Delete to Beginning", "delete_to_start"),
                        SEP(),
                        e("Transpose", "transpose"),
                        e("Trim Trailing Whitespace", "trim"),
                    ],
                ),
                sub(
                    "Code Folding",
                    {
                        let mut items = vec![
                            v("Fold", Target::View("fold"), "fold"),
                            v("Unfold", Target::View("unfold"), "unfold"),
                            v("Unfold All", Target::View("unfold_all"), "unfold_all"),
                            SEP(),
                            v("Fold All", Target::View("fold_all"), "fold_all"),
                        ];
                        items.extend((2..=9).map(|n| Item::new(format!("Fold Level {n}"), Act::Named(format!("fold_level:{n}"))).bound(Target::FoldLevel(n))));
                        items
                    },
                ),
                sub(
                    "Mark",
                    vec![
                        e("Set Mark", "set_mark"),
                        e("Select to Mark", "select_to_mark").enabled(d.editor.has_mark()),
                        e("Delete to Mark", "delete_to_mark").enabled(d.editor.has_mark()),
                        e("Swap with Mark", "swap_mark").enabled(d.editor.has_mark()),
                        e("Clear Mark", "clear_mark").enabled(d.editor.has_mark()),
                        SEP(),
                        e("Yank", "yank").enabled(!self.yank.is_empty()),
                    ],
                ),
                sub(
                    "Convert Case",
                    vec![
                        e("Title Case", "title_case"),
                        e("Upper Case", "upper"),
                        e("Lower Case", "lower"),
                        e("Swap Case", "swap_case"),
                        e("lowerCamelCase", "lower_camel"),
                        e("UpperCamelCase", "upper_camel"),
                        e("snake_case", "snake"),
                        e("kebab-case", "kebab"),
                    ],
                ),
                sub("Wrap", wrap),
                c("Show Completions", Cmd::ShowCompletions),
                SEP(),
                e("Sort Lines", "sort"),
                e("Sort Lines (Case Sensitive)", "sort_case"),
                sub("Permute Lines", vec![e("Reverse", "reverse"), e("Unique", "unique"), e("Shuffle", "shuffle")]),
                sub(
                    "Permute Selections",
                    vec![e("Sort", "sel_sort_ci"), e("Sort (Case Sensitive)", "sel_sort"), e("Reverse", "sel_reverse"), e("Unique", "sel_unique"), e("Shuffle", "sel_shuffle")],
                ),
            ],
        ));

        // ---- Selection
        m.push((
            "Selection".into(),
            vec![
                e("Split into Lines", "split_lines"),
                e("Single Selection", "single_selection"),
                e("Invert Selection", "invert_selection"),
                SEP(),
                Item::new("Select All", Act::SelectAll).bound(Target::Editor("select_all")),
                e("Expand Selection", "expand_selection"),
                e("Expand Selection to Line", "expand_line"),
                e("Expand Selection to Line Upward", "expand_line_up"),
                e("Expand Selection to Word", "expand_word"),
                e("Expand Selection to Block", "expand_block"),
                e("Expand Selection to Paragraph", "expand_paragraph"),
                e("Expand Selection to Brackets", "expand_brackets"),
                e("Expand Selection to Indentation", "expand_indentation"),
                SEP(),
                e("Add Previous Line", "cursor_above"),
                e("Add Next Line", "cursor_below"),
            ],
        ));

        // ---- Find
        m.push((
            "Find".into(),
            vec![
                c("Find…", Cmd::Find),
                c("Find Next", Cmd::FindNext),
                c("Find Previous", Cmd::FindPrev),
                c("Incremental Find", Cmd::IncrementalFind),
                SEP(),
                c("Replace…", Cmd::Replace),
                c("Replace Next", Cmd::ReplaceNext),
                SEP(),
                c("Quick Find", Cmd::QuickFind),
                e("Quick Find All", "all_occurrences"),
                e("Quick Add Next", "next_occurrence"),
                SEP(),
                c("Use Selection for Find", Cmd::UseSelectionForFind),
                c("Use Selection for Replace", Cmd::UseSelectionForReplace),
                SEP(),
                c("Find in Files…", Cmd::FindInFiles),
                c("Replace in Files…", Cmd::ReplaceInFiles),
                sub(
                    "Find Results",
                    vec![
                        Item::new("Show Panel", Act::Named("show_find_results".into())),
                        c("Next Result", Cmd::NextFindResult),
                        c("Previous Result", Cmd::PrevFindResult),
                    ],
                ),
                c("Cancel Find in Files", Cmd::CancelFindInFiles).enabled(self.sidebar.search.running),
            ],
        ));

        // ---- View
        let syntax: Vec<Item> = crate::highlight::syntax_names()
            .into_iter()
            .map(|s| Item::new(s, Act::Syntax(s.to_owned())).checked(d.highlighter.syntax_name() == s))
            .collect();
        let theme = self.theme_menu_items();
        let mut indentation = vec![Item::new("Indent Using Spaces", Act::Named("toggle_spaces".into())).checked(d.editor.use_spaces), SEP()];
        indentation.extend((1..=8).map(|n| Item::new(format!("Tab Width: {n}"), Act::TabWidth(n)).checked(d.editor.tab_width == n)));
        indentation.extend([SEP(), e("Guess Settings From Buffer", "guess_indent"), SEP(), e("Convert Indentation to Spaces", "to_spaces"), e("Convert Indentation to Tabs", "to_tabs")]);
        let ending = d.editor.buffer.line_ending();
        let layout = |label: &str, cmd: Cmd, on: bool| c(label, cmd).checked(on);
        let group_items = |cmd: fn(u8) -> Cmd, next: Cmd, prev: Cmd| {
            let mut v = vec![c("Next", next), c("Previous", prev), SEP()];
            v.extend((0..4u8).map(|g| c(&format!("Group {}", g + 1), cmd(g))));
            v
        };
        m.push((
            "View".into(),
            vec![
                sub("Side Bar", vec![c("Toggle Side Bar", Cmd::ToggleSidebar).checked(self.sidebar.visible), SEP(), c("Toggle Show Open Files", Cmd::ToggleOpenFiles).checked(self.sidebar.show_open_files)]),
                c("Toggle Minimap", Cmd::ToggleMinimap).checked(self.settings.minimap),
                c("Toggle Tabs", Cmd::ToggleTabs).checked(self.settings.show_tabs),
                c("Toggle Status Bar", Cmd::ToggleStatusBar).checked(self.settings.show_status_bar),
                c("Terminal", Cmd::ToggleTerminal).checked(self.terminal.visible),
                sub(
                    "Show Panel",
                    vec![
                        c("Find", Cmd::Find),
                        c("Replace", Cmd::Replace),
                        c("Find in Files", Cmd::FindInFiles),
                        c("Build Results", Cmd::ToggleBuildResults).checked(self.build.visible),
                    ],
                ),
                SEP(),
                c("Toggle Full Screen", Cmd::Fullscreen).checked(self.fullscreen),
                c("Toggle Distraction Free", Cmd::DistractionFree).checked(self.distraction.is_some()),
                c("Markdown Preview", Cmd::ToggleMarkdownPreview).checked(self.settings.markdown_preview),
                SEP(),
                sub(
                    "Layout",
                    vec![
                        layout("Single", Cmd::LayoutSingle, groups == 1),
                        layout("Columns: 2", Cmd::LayoutColumns2, (cols, rows) == (2, 1)),
                        layout("Columns: 3", Cmd::LayoutColumns3, (cols, rows) == (3, 1)),
                        layout("Columns: 4", Cmd::LayoutColumns4, (cols, rows) == (4, 1)),
                        layout("Rows: 2", Cmd::LayoutRows2, (cols, rows) == (1, 2)),
                        layout("Rows: 3", Cmd::LayoutRows3, (cols, rows) == (1, 3)),
                        layout("Grid: 4", Cmd::LayoutGrid, (cols, rows) == (2, 2)),
                    ],
                ),
                sub("Groups", vec![c("Move File to New Group", Cmd::MoveToNewGroup), c("New Group", Cmd::NewGroup), c("Close Group", Cmd::CloseGroup).enabled(groups > 1)]),
                sub("Focus Group", group_items(Cmd::FocusGroup, Cmd::FocusNextGroup, Cmd::FocusPrevGroup)),
                sub("Move File to Group", group_items(Cmd::MoveToGroup, Cmd::MoveToNextGroup, Cmd::MoveToPrevGroup)),
                SEP(),
                sub("Syntax", syntax),
                sub("Indentation", indentation),
                sub(
                    "Line Endings",
                    vec![
                        Item::new("Windows", Act::Named("crlf".into())).checked(ending == crate::buffer::LineEnding::CrLf),
                        Item::new("Unix", Act::Named("lf".into())).checked(ending == crate::buffer::LineEnding::Lf),
                    ],
                ),
                c("Word Wrap", Cmd::ToggleWordWrap).checked(self.wraps(self.active)),
                sub("Word Wrap Column", {
                    let mut v = vec![Item::new("Automatic", Act::Named("wrapcol:auto".into())).checked(self.settings.wrap_column.is_none()), SEP()];
                    v.extend(wraps.iter().map(|w| Item::new(w.to_string(), Act::Named(format!("wrapcol:{w}"))).checked(self.settings.wrap_column == Some(*w))));
                    v
                }),
                sub("Ruler", {
                    let mut v = vec![Item::new("None", Act::Named("ruler:none".into())).checked(self.settings.rulers.is_empty()), SEP()];
                    v.extend(wraps.iter().map(|w| Item::new(w.to_string(), Act::Named(format!("ruler:{w}"))).checked(self.settings.rulers.contains(w))));
                    v
                }),
                SEP(),
                sub("Color Scheme", theme),
                sub("Editor Extras", {
                    let s = &self.settings;
                    let t = |label: &str, key: &str, on: bool| Item::new(label, Act::Named(format!("x:{key}"))).checked(on);
                    vec![
                        t("Show Whitespace in Selection", "ws_sel", s.draw_white_space == "selection"),
                        t("Show All Whitespace", "ws_all", s.draw_white_space == "all"),
                        t("Hide Whitespace", "ws_none", s.draw_white_space == "none"),
                        t("Highlight Trailing Whitespace", "trailing", s.highlight_trailing_whitespace),
                        SEP(),
                        t("Bracket Pair Colors", "brackets", s.bracket_colors),
                        t("Color Swatches", "swatches", s.color_swatches),
                        t("Sticky Scroll", "sticky", s.sticky_scroll),
                        t("Smooth Scrolling", "smooth", s.smooth_scroll),
                        t("Git Change Markers", "git", s.git_gutter),
                    ]
                }),
                sub(
                    "Cursor",
                    vec![
                        Item::new("Line", Act::Named("caret:line".into())).checked(self.settings.caret_style == "line"),
                        Item::new("Block", Act::Named("caret:block".into())).checked(self.settings.caret_style == "block"),
                        Item::new("Underline", Act::Named("caret:underline".into())).checked(self.settings.caret_style == "underline"),
                        SEP(),
                        Item::new("Blink", Act::Named("blink:blink".into())).checked(self.settings.caret_blink == "blink"),
                        Item::new("Smooth Fade", Act::Named("blink:smooth".into())).checked(self.settings.caret_blink == "smooth"),
                        Item::new("Solid", Act::Named("blink:solid".into())).checked(self.settings.caret_blink == "solid"),
                        SEP(),
                        Item::new("Thin", Act::Named("caretw:1".into())).checked(self.settings.caret_width == 1.0),
                        Item::new("Normal", Act::Named("caretw:2".into())).checked(self.settings.caret_width == 2.0),
                        Item::new("Thick", Act::Named("caretw:3".into())).checked(self.settings.caret_width == 3.0),
                    ],
                ),
                sub(
                    "Window Opacity",
                    [100, 95, 90, 85, 80, 70, 60]
                        .into_iter()
                        .map(|p| Item::new(format!("{p}%"), Act::Named(format!("opacity:{p}"))).checked((self.settings.window_opacity * 100.0).round() as i32 == p))
                        .collect(),
                ),
            ],
        ));

        // ---- Goto
        let mut bookmarks = vec![
            e("Toggle Bookmark", "bookmark_toggle"),
            e("Next Bookmark", "bookmark_next"),
            e("Prev Bookmark", "bookmark_prev"),
            e("Clear Bookmarks", "bookmark_clear"),
            e("Select All Bookmarks", "bookmark_select"),
            SEP(),
        ];
        if d.editor.bookmarks().is_empty() {
            bookmarks.push(Item::new("(No Bookmarks)", Act::Named(String::new())).enabled(false));
        }
        for &pos in d.editor.bookmarks().iter().take(30) {
            let line = d.editor.buffer.line_of(pos);
            let text: String = d.editor.buffer.line_text(line).trim().chars().take(50).collect();
            bookmarks.push(Item::new(format!("{}: {text}", line + 1), Act::Named(format!("bm:{pos}"))));
        }
        m.push((
            "Goto".into(),
            vec![
                c("Goto Anything…", Cmd::GotoAnything),
                SEP(),
                c("Goto Symbol…", Cmd::GotoSymbol),
                c("Goto Symbol in Project…", Cmd::GotoSymbolInProject),
                c("Goto Definition…", Cmd::GotoDefinition),
                c("Goto Reference…", Cmd::GotoReference),
                c("Goto Line…", Cmd::GotoLine),
                SEP(),
                c("Next Modification", Cmd::NextModification),
                c("Previous Modification", Cmd::PrevModification),
                SEP(),
                c("Jump Back", Cmd::JumpBack).enabled(!self.jumps_back.is_empty()),
                c("Jump Forward", Cmd::JumpForward).enabled(!self.jumps_forward.is_empty()),
                SEP(),
                sub(
                    "Switch File",
                    vec![
                        c("Next File", Cmd::NextTab),
                        c("Previous File", Cmd::PrevTab),
                        SEP(),
                        c("Next File in Stack", Cmd::NextInStack),
                        c("Previous File in Stack", Cmd::PrevInStack),
                        SEP(),
                        c("Switch Header/Implementation", Cmd::SwitchHeader),
                    ],
                ),
                SEP(),
                sub(
                    "Scroll",
                    vec![
                        v("Scroll to Selection", Target::View("scroll_sel"), "scroll_sel"),
                        v("Line Up", Target::View("scroll_up"), "scroll_up"),
                        v("Line Down", Target::View("scroll_down"), "scroll_down"),
                    ],
                ),
                sub("Bookmarks", bookmarks),
                SEP(),
                e("Jump to Matching Bracket", "jump_bracket"),
            ],
        ));

        // ---- Tools
        let running = self.build.is_running();
        let mut systems = vec![Item::new("Automatic", Act::Named("bs:".into())).checked(self.build_system.is_none()), SEP()];
        let mut names: Vec<&str> = self.build.systems.systems.iter().map(|s| s.name.as_str()).collect();
        names.sort_unstable_by_key(|n| n.to_lowercase());
        names.dedup();
        systems.extend(names.into_iter().map(|n| Item::new(n, Act::Named(format!("bs:{n}"))).checked(self.build_system.as_deref() == Some(n))));
        systems.extend([SEP(), c("New Build System…", Cmd::NewBuildSystem).enabled(self.config.is_some())]);
        m.push((
            "Tools".into(),
            vec![
                c("Command Palette…", Cmd::CommandPalette),
                c("Snippets…", Cmd::Snippets),
                SEP(),
                sub("Build System", systems),
                c("Build", Cmd::Build),
                c("Build With…", Cmd::BuildWith),
                c("Cancel Build", Cmd::CancelBuild).enabled(running),
                sub(
                    "Build Results",
                    vec![
                        c("Show Build Results", Cmd::ToggleBuildResults).checked(self.build.visible),
                        c("Next Result", Cmd::NextResult),
                        c("Previous Result", Cmd::PrevResult),
                    ],
                ),
                c("Save All on Build", Cmd::ToggleSaveAllOnBuild).checked(self.settings.save_all_on_build),
                SEP(),
                c("New Terminal", Cmd::NewTerminal),
                SEP(),
                c(if self.macro_recording.is_some() { "Stop Recording Macro" } else { "Record Macro" }, Cmd::RecordMacro),
                c("Playback Macro", Cmd::PlaybackMacro).enabled(!self.macro_last.is_empty()),
                SEP(),
                sub("Developer", vec![c("New Snippet…", Cmd::NewSnippet)]),
                SEP(),
                sub(
                    "Text",
                    vec![
                        e("Format JSON", "tx_json_format"),
                        e("Minify JSON", "tx_json_minify"),
                        e("Sort JSON Keys", "tx_json_sort"),
                        SEP(),
                        e("Base64 Encode", "tx_b64_encode"),
                        e("Base64 Decode", "tx_b64_decode"),
                        e("URL Encode", "tx_url_encode"),
                        e("URL Decode", "tx_url_decode"),
                        e("HTML Escape", "tx_html_escape"),
                        e("HTML Unescape", "tx_html_unescape"),
                        SEP(),
                        e("Insert UUID", "tx_uuid"),
                        e("Insert Date and Time", "tx_date"),
                    ],
                ),
                sub(
                    "On Save",
                    vec![
                        Item::new("Trim Trailing Whitespace", Act::Named("opt:trim".into())).checked(self.settings.trim_trailing_white_space_on_save),
                        Item::new("Ensure Newline at End of File", Act::Named("opt:eof".into())).checked(self.settings.ensure_newline_at_eof_on_save),
                        Item::new("Save When Window Loses Focus", Act::Named("opt:focus".into())).checked(self.settings.save_on_focus_lost),
                    ],
                ),
            ],
        ));

        // ---- Project
        let mut recent_projects: Vec<Item> = self.recent_projects.iter().map(|p| Item::new(p.display().to_string(), Act::Named(format!("project:{}", p.display())))).collect();
        recent_projects.extend(self.recent_folders.iter().map(|p| Item::new(format!("{}/", p.display()), Act::OpenRecentFolder(p.clone()))));
        recent_projects.extend([SEP(), c("Clear Items", Cmd::ClearRecent)]);
        let folder = self.sidebar.root.is_some();
        m.push((
            "Project".into(),
            vec![
                c("Open Project…", Cmd::OpenProject),
                c("Switch Project…", Cmd::SwitchProject),
                sub("Open Recent", recent_projects),
                SEP(),
                c("Save Project As…", Cmd::SaveProjectAs),
                c("Close Project", Cmd::CloseProject).enabled(folder || self.project_path.is_some()),
                c("Edit Project", Cmd::EditProject).enabled(self.project_path.is_some()),
                SEP(),
                c("Add Folder to Project…", Cmd::OpenFolder),
                c("Remove all Folders from Project", Cmd::CloseFolder).enabled(folder),
                c("Refresh Folders", Cmd::RefreshFolders).enabled(folder),
            ],
        ));

        if !MAC {
            m.push(("Preferences".into(), settings_owned(self)));
        }

        // ---- Window (macOS adds its own window-management items)
        let names = self.display_names();
        let mut window = Vec::new();
        if MAC {
            window.extend([Item::System(System::Minimize), Item::System(System::Zoom), SEP()]);
        }
        window.extend(names.into_iter().enumerate().map(|(i, n)| Item::new(n, Act::Tab(i)).checked(i == self.active)));
        // Every window, to bring one forward.
        if self.link.windows.len() > 1 {
            window.push(SEP());
            window.extend(self.link.windows.iter().enumerate().map(|(k, t)| Item::new(t, Act::Named(format!("window:{k}"))).checked(k == self.link.index)));
        }
        if MAC {
            window.extend([SEP(), Item::System(System::BringAllToFront)]);
        }
        m.push(("Window".into(), window));

        let mut help = vec![c("Documentation", Cmd::Documentation), SEP(), c("Licenses and Attribution…", Cmd::Licenses)];
        if !MAC {
            help.extend([SEP(), Item::new("About Strata", Act::Cmd(Cmd::About))]);
        }
        m.push(("Help".into(), help));
        m
    }

    /// Carry out a menu choice.
    pub(super) fn run_act(&mut self, ctx: &egui::Context, act: crate::menus::Act) {
        use egui::Event;
        let cmd_mods = if MAC { Modifiers::MAC_CMD.plus(Modifiers::COMMAND) } else { Modifiers::CTRL.plus(Modifiers::COMMAND) };
        let key = |key: Key, modifiers: Modifiers| Event::Key { key, physical_key: Some(key), pressed: true, repeat: false, modifiers };
        let editor_focused = self.docs[self.active].view.has_focus(ctx);
        match act {
            // The editor's own commands, so they follow its key bindings; text fields get the keys.
            Act::Undo if editor_focused => self.run_editor("undo"),
            Act::Redo if editor_focused => self.run_editor("redo"),
            Act::SelectAll if editor_focused => self.run_editor("select_all"),
            Act::Cmd(cmd) => self.run(ctx, cmd),
            Act::Editor(id) => self.run_editor(id),
            // These go through egui's input so the focused widget (editor or a text field) gets them.
            Act::Copy => self.pending_events.push(Event::Copy),
            Act::Cut => self.pending_events.push(Event::Cut),
            Act::Paste => {
                if let Ok(text) = arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
                    self.pending_events.push(Event::Paste(text));
                }
            }
            Act::Undo => self.pending_events.push(key(Key::Z, cmd_mods)),
            Act::Redo => self.pending_events.push(if MAC { key(Key::Z, cmd_mods.plus(Modifiers::SHIFT)) } else { key(Key::Y, cmd_mods) }),
            Act::SelectAll => self.pending_events.push(key(Key::A, cmd_mods)),
            Act::OpenRecent(p) => self.open_path(p, false),
            Act::OpenRecentFolder(p) => self.open_folder(p),
            Act::Syntax(s) => self.doc().highlighter.set_syntax(&s),
            Act::Theme(t) => self.pick_theme(t),
            // Like Sublime, these apply to the current file.
            Act::TabWidth(n) => self.doc().editor.tab_width = n,
            Act::Tab(i) if i < self.docs.len() => self.activate(i),
            Act::Tab(_) => {}
            Act::ReopenWithEncoding(e) => self.reopen_with_encoding(e),
            Act::SaveWithEncoding(e) => self.save_with_encoding(e),
            Act::Quit => self.quit(ctx),
            Act::Named(n) => match n.as_str() {
                "toggle_spaces" => {
                    let e = &mut self.doc().editor;
                    e.use_spaces = !e.use_spaces;
                }
                "lf" => self.doc().set_line_ending(crate::buffer::LineEnding::Lf),
                "fold" | "unfold" | "unfold_all" | "fold_all" | "scroll_sel" | "scroll_up" | "scroll_down" => {
                    let d = self.doc();
                    match n.as_str() {
                        "fold" => d.view.fold(&mut d.editor),
                        "unfold" => d.view.unfold(&mut d.editor),
                        "unfold_all" => d.view.unfold_all(),
                        "fold_all" => d.view.fold_all(&mut d.editor),
                        "scroll_sel" => d.view.center_on_cursor(),
                        "scroll_up" => d.view.scroll_lines(-1),
                        _ => d.view.scroll_lines(1),
                    }
                }
                n if n.starts_with("fold_level:") => {
                    let level = n[11..].parse().unwrap_or(2);
                    let d = self.doc();
                    d.view.fold_level(&mut d.editor, level);
                }
                "opt:trim" | "opt:eof" | "opt:focus" => {
                    let s = &mut self.settings;
                    let flag = match n.as_str() {
                        "opt:trim" => &mut s.trim_trailing_white_space_on_save,
                        "opt:eof" => &mut s.ensure_newline_at_eof_on_save,
                        _ => &mut s.save_on_focus_lost,
                    };
                    *flag = !*flag;
                    self.persist_settings();
                }
                n if n.starts_with("x:") => {
                    let s = &mut self.settings;
                    match &n[2..] {
                        "ws_sel" => s.draw_white_space = "selection".into(),
                        "ws_all" => s.draw_white_space = "all".into(),
                        "ws_none" => s.draw_white_space = "none".into(),
                        "trailing" => s.highlight_trailing_whitespace = !s.highlight_trailing_whitespace,
                        "brackets" => s.bracket_colors = !s.bracket_colors,
                        "swatches" => s.color_swatches = !s.color_swatches,
                        "sticky" => s.sticky_scroll = !s.sticky_scroll,
                        "smooth" => s.smooth_scroll = !s.smooth_scroll,
                        "git" => s.git_gutter = !s.git_gutter,
                        _ => {}
                    }
                    self.persist_settings();
                }
                n if n.starts_with("caret:") => {
                    self.settings.caret_style = n[6..].to_owned();
                    self.persist_settings();
                }
                n if n.starts_with("blink:") => {
                    self.settings.caret_blink = n[6..].to_owned();
                    self.persist_settings();
                }
                n if n.starts_with("caretw:") => {
                    self.settings.caret_width = n[7..].parse().unwrap_or(2.0);
                    self.persist_settings();
                }
                n if n.starts_with("opacity:") => {
                    self.settings.window_opacity = n[8..].parse::<f32>().unwrap_or(100.0) / 100.0;
                    self.persist_settings();
                }
                "appearance:auto" | "appearance:fixed" => self.set_follow_system(n == "appearance:auto"),
                n if n.starts_with("linespacing:") => {
                    self.settings.line_height = n[12..].parse().unwrap_or(1.0);
                    self.settings.line_padding_top = 0.0;
                    self.settings.line_padding_bottom = 0.0;
                    self.persist_settings();
                }
                n if n.starts_with("uisize:") => {
                    self.settings.ui_font_size = n[7..].parse().unwrap_or(13.0);
                    self.persist_settings();
                }
                n if n.starts_with("wrapcol:") => {
                    self.settings.wrap_column = n[8..].parse().ok();
                    self.persist_settings();
                }
                n if n.starts_with("ruler:") => {
                    self.settings.rulers = n[6..].parse().into_iter().collect();
                    self.persist_settings();
                }
                "show_find_results" => {
                    self.sidebar.visible = true;
                    self.sidebar.tab = crate::sidebar::Tab::Search;
                }
                n if n.starts_with("wrap:") => {
                    let width = n[5..].parse().unwrap_or(80);
                    self.doc().editor.wrap_paragraph(width);
                }
                n if n.starts_with("bm:") => {
                    let pos: usize = n[3..].parse().unwrap_or(0);
                    let d = self.doc();
                    let pos = pos.min(d.editor.buffer.len_chars());
                    d.editor.select_ranges(&[pos..pos], 0);
                    d.view.reveal_centered();
                }
                n if n.starts_with("bs:") => self.build_system = Some(n[3..].to_owned()).filter(|s| !s.is_empty()),
                n if n.starts_with("project:") => self.open_project_path(std::path::PathBuf::from(&n[8..])),
                n if n.starts_with("window:") => self.link.requests.focus_window = n[7..].parse().ok(),
                "crlf" => self.doc().set_line_ending(crate::buffer::LineEnding::CrLf),
                _ => {}
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_menu_item_refers_to_something_real() {
        let app = StrataApp::new(None, None);
        let menus = app.menus();
        assert!(menus.iter().any(|(n, _)| n == "Selection"));
        fn walk(items: &[Item], n: &mut usize) {
            for it in items {
                match it {
                    Item::Action { act: Act::Editor(id), .. } => {
                        find(id);
                        *n += 1;
                    }
                    Item::Action { .. } => *n += 1,
                    Item::Sub(_, kids) => walk(kids, n),
                    _ => {}
                }
            }
        }
        let mut n = 0;
        for (_, items) in &menus {
            walk(items, &mut n);
        }
        assert!(n > 100, "{n} items");
    }

    #[test]
    fn editor_shortcuts_are_unique() {
        let mut seen = Vec::new();
        for c in EDITOR_COMMANDS {
            for &(m, k) in c.keys {
                let l = label(m, k);
                assert!(!seen.contains(&l), "{l} bound twice");
                seen.push(l);
            }
        }
    }
}
