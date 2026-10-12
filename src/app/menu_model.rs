//! The menu bar's contents (Sublime's layout: File, Edit, Selection, Find,
//! View, Goto, Tools, Project, Window, Help) and the editor-command table
//! shared by menus, palette and shortcuts.

use super::StrataApp;
use crate::commands::Cmd;
use crate::keys::*;
use crate::menus::{Act, Item, Menus, System};
use egui::{Key, Modifiers};

/// An editor command: id (for `run_editor`), palette name, shortcuts, and a
/// chord shown as text where a single shortcut can't express it.
pub struct EdCmd {
    pub id: &'static str,
    pub name: &'static str,
    pub keys: &'static [(Modifiers, Key)],
    pub chord: Option<&'static str>,
}

const fn ed(id: &'static str, name: &'static str, keys: &'static [(Modifiers, Key)]) -> EdCmd {
    EdCmd { id, name, keys, chord: None }
}

const fn chord(id: &'static str, name: &'static str, chord: &'static str) -> EdCmd {
    EdCmd { id, name, keys: &[], chord: Some(chord) }
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
    chord("upper", "Edit: Convert Case: Upper Case", if MAC { "⌘K ⌘U" } else { "Ctrl+K Ctrl+U" }),
    chord("lower", "Edit: Convert Case: Lower Case", if MAC { "⌘K ⌘L" } else { "Ctrl+K Ctrl+L" }),
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
    ed("redo", "Edit: Redo", if MAC { &[(CMD_SHIFT, Key::Z)] } else { &[(CMD, Key::Y)] }),
];

fn find(id: &str) -> &'static EdCmd {
    EDITOR_COMMANDS.iter().find(|c| c.id == id).unwrap_or_else(|| panic!("unknown editor command {id}"))
}

/// (id, name, shortcut label) for the command palette.
pub fn palette_editor_commands() -> Vec<(&'static str, &'static str, String)> {
    EDITOR_COMMANDS
        .iter()
        .map(|c| {
            let keys = c.chord.map(spaced).or_else(|| c.keys.first().map(|&(m, k)| label(m, k))).unwrap_or_default();
            (c.id, c.name, keys)
        })
        .collect()
}

/// A menu item for an editor command, labeled `label`.
fn e(label: &str, id: &'static str) -> Item {
    let c = find(id);
    let item = Item::new(label, Act::Editor(id)).keys(c.keys.first().copied());
    match c.chord {
        Some(ch) => item.hint(ch),
        None => item,
    }
}

fn c(label: &str, cmd: Cmd) -> Item {
    Item::cmd(label, cmd)
}

fn sub(label: &str, items: Vec<Item>) -> Item {
    Item::Sub(label.into(), items)
}

const SEP: fn() -> Item = || Item::Separator;

impl StrataApp {
    /// The whole menu bar for the current state (checks, recent files, tabs).
    pub(super) fn menus(&self) -> Menus {
        let d = &self.docs[self.active];
        let groups = self.layout.groups();
        let (cols, rows) = (self.layout.cols.len(), self.layout.rows.len());
        let mut m: Menus = Vec::new();

        if MAC {
            m.push((
                "Strata".into(),
                vec![
                    Item::new("About Strata", Act::Cmd(Cmd::About)),
                    SEP(),
                    sub(
                        "Settings",
                        vec![c("Settings", Cmd::Settings), c("Color Scheme…", Cmd::SelectTheme)],
                    ),
                    SEP(),
                    Item::System(System::Services),
                    SEP(),
                    Item::System(System::Hide),
                    Item::System(System::HideOthers),
                    Item::System(System::ShowAll),
                    SEP(),
                    Item::new("Quit Strata", Act::Quit).key(CMD, Key::Q),
                ],
            ));
        }

        let mut recent: Vec<Item> = self.recent_files.iter().map(|p| Item::new(p.display().to_string(), Act::OpenRecent(p.clone()))).collect();
        if !recent.is_empty() {
            recent.push(SEP());
        }
        recent.extend(self.recent_folders.iter().map(|p| Item::new(format!("{}/", p.display()), Act::OpenRecentFolder(p.clone()))));
        recent.push(SEP());
        recent.push(c("Reopen Closed File", Cmd::ReopenClosed).enabled(!self.closed.is_empty()));
        recent.push(c("Clear Items", Cmd::ClearRecent));
        let mut file = vec![
            c("New File", Cmd::NewTab),
            SEP(),
            c("Open…", Cmd::Open),
            c("Open Folder…", Cmd::OpenFolder),
            sub("Open Recent", recent),
            SEP(),
            sub("Reopen with Encoding", crate::encoding::Encoding::all().iter().map(|e| Item::new(e.name(), Act::ReopenWithEncoding(*e)).checked(d.encoding == *e)).collect()),
            SEP(),
            c("Save", Cmd::Save),
            sub("Save with Encoding", crate::encoding::Encoding::all().iter().map(|e| Item::new(e.name(), Act::SaveWithEncoding(*e)).checked(d.encoding == *e)).collect()),
            c("Save As…", Cmd::SaveAs),
            c("Save All", Cmd::SaveAll),
            SEP(),
            c("Revert File", Cmd::RevertFile).enabled(d.path.is_some()),
            SEP(),
            c("Close File", Cmd::CloseTab),
            c("Close All Files", Cmd::CloseAll),
            c("Close Window", Cmd::CloseWindow),
        ];
        if !MAC {
            file.push(SEP());
            file.push(Item::new("Exit", Act::Quit));
        }
        m.push(("File".into(), file));

        m.push((
            "Edit".into(),
            vec![
                Item::new("Undo", Act::Undo).key(CMD, Key::Z),
                Item::new("Redo", Act::Redo).keys(find("redo").keys.first().copied()),
                SEP(),
                Item::new("Cut", Act::Cut).key(CMD, Key::X),
                Item::new("Copy", Act::Copy).key(CMD, Key::C),
                Item::new("Paste", Act::Paste).key(CMD, Key::V),
                e("Paste and Indent", "paste_and_indent"),
                SEP(),
                sub(
                    "Line",
                    vec![
                        e("Indent", "indent"),
                        e("Unindent", "outdent"),
                        SEP(),
                        e("Swap Line Up", "move_up"),
                        e("Swap Line Down", "move_down"),
                        e("Duplicate Line", "duplicate"),
                        e("Delete Line", "delete_line"),
                        e("Join Lines", "join"),
                        e("Reindent", "reindent"),
                    ],
                ),
                sub("Comment", vec![e("Toggle Comment", "toggle_comment")]),
                sub(
                    "Text",
                    vec![
                        e("Insert Line Before", "line_before"),
                        e("Insert Line After", "line_after"),
                        SEP(),
                        e("Delete to End", "delete_to_end"),
                        e("Delete to Beginning", "delete_to_start"),
                        SEP(),
                        e("Transpose", "transpose"),
                        e("Trim Trailing Whitespace", "trim"),
                    ],
                ),
                sub(
                    "Bookmarks",
                    vec![
                        e("Toggle Bookmark", "bookmark_toggle"),
                        e("Next Bookmark", "bookmark_next"),
                        e("Previous Bookmark", "bookmark_prev"),
                        e("Clear Bookmarks", "bookmark_clear"),
                        e("Select All Bookmarks", "bookmark_select"),
                    ],
                ),
                sub("Convert Case", vec![e("Title Case", "title_case"), e("Upper Case", "upper"), e("Lower Case", "lower"), e("Swap Case", "swap_case")]),
                SEP(),
                e("Wrap Paragraph at Ruler", "wrap_paragraph"),
                c("Show Completions", Cmd::ShowCompletions),
                SEP(),
                e("Sort Lines", "sort"),
                e("Sort Lines (Case Sensitive)", "sort_case"),
                sub("Permute Lines", vec![e("Reverse", "reverse"), e("Unique", "unique"), e("Shuffle", "shuffle")]),
                sub(
                    "Permute Selections",
                    vec![e("Sort", "sel_sort"), e("Reverse", "sel_reverse"), e("Unique", "sel_unique"), e("Shuffle", "sel_shuffle")],
                ),
            ],
        ));

        m.push((
            "Selection".into(),
            vec![
                e("Split into Lines", "split_lines"),
                e("Add Previous Line", "cursor_above"),
                e("Add Next Line", "cursor_below"),
                e("Single Selection", "single_selection"),
                e("Invert Selection", "invert_selection"),
                SEP(),
                Item::new("Select All", Act::SelectAll).key(CMD, Key::A),
                e("Expand Selection to Line", "expand_line"),
                e("Expand Selection to Word", "expand_word"),
                e("Expand Selection to Paragraph", "expand_paragraph"),
                e("Expand Selection to Brackets", "expand_brackets"),
                e("Expand Selection to Indentation", "expand_indentation"),
            ],
        ));

        m.push((
            "Find".into(),
            vec![
                c("Find…", Cmd::Find),
                c("Find Next", Cmd::FindNext),
                c("Find Previous", Cmd::FindPrev),
                c("Incremental Find", Cmd::IncrementalFind),
                SEP(),
                c("Replace…", Cmd::Replace),
                SEP(),
                c("Quick Find", Cmd::QuickFind),
                e("Quick Find All", "all_occurrences"),
                e("Quick Add Next", "next_occurrence"),
                SEP(),
                c("Use Selection for Find", Cmd::UseSelectionForFind),
                c("Use Selection for Replace", Cmd::UseSelectionForReplace),
                SEP(),
                c("Find in Files…", Cmd::FindInFiles),
            ],
        ));

        let layout = |label: &str, cmd: Cmd, on: bool| c(label, cmd).checked(on);
        let syntax: Vec<Item> = crate::highlight::syntax_names()
            .into_iter()
            .map(|s| Item::new(s, Act::Syntax(s.to_owned())).checked(d.highlighter.syntax_name() == s))
            .collect();
        let theme: Vec<Item> = crate::highlight::theme_names()
            .into_iter()
            .map(|t| Item::new(t, Act::Theme(t.to_owned())).checked(self.settings.theme == t))
            .collect();
        let mut indentation = vec![Item::new("Indent Using Spaces", Act::Named("toggle_spaces".into())).checked(self.settings.translate_tabs_to_spaces), SEP()];
        indentation.extend((1..=8).map(|n| Item::new(format!("Tab Width: {n}"), Act::TabWidth(n)).checked(self.settings.tab_size == n)));
        indentation.extend([SEP(), e("Guess Settings From Buffer", "guess_indent"), SEP(), e("Convert Indentation to Spaces", "to_spaces"), e("Convert Indentation to Tabs", "to_tabs")]);
        let ending = d.editor.buffer.line_ending();
        m.push((
            "View".into(),
            vec![
                sub(
                    "Side Bar",
                    vec![c(if self.sidebar.visible { "Hide Side Bar" } else { "Show Side Bar" }, Cmd::ToggleSidebar)],
                ),
                c(if self.settings.minimap { "Hide Minimap" } else { "Show Minimap" }, Cmd::ToggleMinimap),
                c(if self.settings.show_tabs { "Hide Tabs" } else { "Show Tabs" }, Cmd::ToggleTabs),
                c(if self.settings.show_status_bar { "Hide Status Bar" } else { "Show Status Bar" }, Cmd::ToggleStatusBar),
                SEP(),
                c(if self.fullscreen { "Exit Full Screen" } else { "Enter Full Screen" }, Cmd::Fullscreen),
                c(if self.distraction.is_some() { "Exit Distraction Free Mode" } else { "Enter Distraction Free Mode" }, Cmd::DistractionFree),
                SEP(),
                sub(
                    "Layout",
                    vec![
                        layout("Single", Cmd::LayoutSingle, groups == 1),
                        layout("Columns: 2", Cmd::LayoutColumns2, (cols, rows) == (2, 1)),
                        layout("Columns: 3", Cmd::LayoutColumns3, (cols, rows) == (3, 1)),
                        layout("Columns: 4", Cmd::LayoutColumns4, (cols, rows) == (4, 1)),
                        layout("Rows: 2", Cmd::LayoutRows2, (cols, rows) == (1, 2)),
                        layout("Grid: 4", Cmd::LayoutGrid, (cols, rows) == (2, 2)),
                    ],
                ),
                sub("Focus Group", (0..4u8).map(|g| c(&format!("Group {}", g + 1), Cmd::FocusGroup(g)).enabled((g as usize) < groups)).collect()),
                sub("Move File to Group", (0..4u8).map(|g| c(&format!("Group {}", g + 1), Cmd::MoveToGroup(g))).collect()),
                SEP(),
                sub("Syntax", syntax),
                sub("Indentation", indentation),
                sub(
                    "Line Endings",
                    vec![
                        Item::new("Unix (LF)", Act::Named("lf".into())).checked(ending == crate::buffer::LineEnding::Lf),
                        Item::new("Windows (CRLF)", Act::Named("crlf".into())).checked(ending == crate::buffer::LineEnding::CrLf),
                    ],
                ),
                SEP(),
                c("Word Wrap", Cmd::ToggleWordWrap).checked(self.wraps(self.active)),
                SEP(),
                sub("Color Scheme", theme),
                c("Zoom In", Cmd::ZoomIn),
                c("Zoom Out", Cmd::ZoomOut),
                c("Reset Zoom", Cmd::ZoomReset),
            ],
        ));

        m.push((
            "Goto".into(),
            vec![
                c("Goto Anything…", Cmd::GotoAnything),
                SEP(),
                c("Goto Symbol…", Cmd::GotoSymbol),
                c("Goto Definition…", Cmd::GotoDefinition),
                c("Goto Line…", Cmd::GotoLine),
                SEP(),
                c("Jump Back", Cmd::JumpBack).enabled(!self.jumps_back.is_empty()),
                c("Jump Forward", Cmd::JumpForward).enabled(!self.jumps_forward.is_empty()),
                SEP(),
                sub("Switch File", vec![c("Next File", Cmd::NextTab), c("Previous File", Cmd::PrevTab)]),
                e("Jump to Matching Bracket", "jump_bracket"),
                SEP(),
                sub(
                    "Bookmarks",
                    vec![
                        e("Toggle Bookmark", "bookmark_toggle"),
                        e("Next Bookmark", "bookmark_next"),
                        e("Previous Bookmark", "bookmark_prev"),
                        e("Clear Bookmarks", "bookmark_clear"),
                        e("Select All Bookmarks", "bookmark_select"),
                    ],
                ),
            ],
        ));

        let running = self.build.is_running();
        m.push((
            "Tools".into(),
            vec![
                c("Command Palette…", Cmd::CommandPalette),
                SEP(),
                c("Build", Cmd::Build),
                c("Build With…", Cmd::BuildWith),
                c("Cancel Build", Cmd::CancelBuild).enabled(running),
                sub(
                    "Build Results",
                    vec![
                        c(if self.build.visible { "Hide Build Results" } else { "Show Build Results" }, Cmd::ToggleBuildResults),
                        c("Next Result", Cmd::NextResult),
                        c("Previous Result", Cmd::PrevResult),
                    ],
                ),
            ],
        ));

        let mut folders: Vec<Item> = self.recent_folders.iter().map(|p| Item::new(p.display().to_string(), Act::OpenRecentFolder(p.clone()))).collect();
        if folders.is_empty() {
            folders.push(Item::new("(none yet)", Act::Named(String::new())).enabled(false));
        }
        m.push((
            "Project".into(),
            vec![
                c("Open Project…", Cmd::OpenProject),
                sub("Open Recent", folders),
                c("Save Project As…", Cmd::SaveProjectAs),
                SEP(),
                c("Add Folder to Project…", Cmd::OpenFolder),
                c("Close Folder", Cmd::CloseFolder).enabled(self.sidebar.root.is_some()),
                c("Refresh Folders", Cmd::RefreshFolders).enabled(self.sidebar.root.is_some()),
            ],
        ));

        let names = self.display_names();
        let mut window = Vec::new();
        if MAC {
            window.extend([Item::System(System::Minimize), Item::System(System::Zoom), SEP()]);
        }
        window.extend(names.into_iter().enumerate().map(|(i, n)| Item::new(n, Act::Tab(i)).checked(i == self.active)));
        if MAC {
            window.extend([SEP(), Item::System(System::BringAllToFront)]);
        }
        m.push(("Window".into(), window));

        let mut help = vec![c("Documentation", Cmd::Documentation), c("Command Palette…", Cmd::CommandPalette)];
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
        match act {
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
            Act::Theme(t) => {
                self.settings.theme = t;
                self.apply_settings();
                self.persist_settings();
            }
            Act::TabWidth(n) => {
                self.settings.tab_size = n;
                self.apply_settings();
                self.persist_settings();
            }
            Act::Tab(i) if i < self.docs.len() => self.activate(i),
            Act::Tab(_) => {}
            Act::ReopenWithEncoding(e) => self.reopen_with_encoding(e),
            Act::SaveWithEncoding(e) => self.save_with_encoding(e),
            Act::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Act::Named(n) => match n.as_str() {
                "toggle_spaces" => {
                    self.settings.translate_tabs_to_spaces = !self.settings.translate_tabs_to_spaces;
                    self.apply_settings();
                    self.persist_settings();
                }
                "lf" => self.doc().set_line_ending(crate::buffer::LineEnding::Lf),
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
