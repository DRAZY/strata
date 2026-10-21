//! User-customizable key bindings, Sublime Text style.
//!
//! The built-in tables (`Cmd::default_keys`, the editor command table and a
//! few view commands below) make the default keymap. `keybindings.json` in the
//! config folder is a JSON array (comments and trailing commas allowed) of
//!
//! ```text
//! { "keys": ["primary+shift+k"], "command": "delete_line" }
//! { "keys": ["super+k", "super+u"], "command": "upper_case" }          // a chord
//! { "keys": ["super+shift+p"], "command": "show_overlay", "args": {"overlay": "command_palette"} }
//! { "keys": ["ctrl+`"], "command": "toggle_terminal", "context": [{"key": "terminal_focus"}] }
//! { "keys": ["super+shift+d"], "command": "" }                         // unbind
//! ```
//!
//! Commands go by Strata's ids (`Cmd::id`, the editor table's public ids) or
//! by the Sublime command name they correspond to, args included, so pasted
//! Sublime keymaps mostly just work. Later entries win; binding a command
//! replaces its default keys; `""` or `"unbound"` removes whatever a key had.
//!
//! The resolved `Keymap` is the one source for every shortcut: window and
//! editor dispatch (including Cmd+K chords), menu labels and native menu
//! accelerators, the command palette and context menus.

use crate::app::menu_model::EDITOR_COMMANDS;
use crate::commands::Cmd;
use crate::keys::{self, *};
use egui::{Event, Key, Modifiers};
use serde_json::{Map, Value};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::SystemTime;

/// The user's file, in the config folder.
pub const FILE_NAME: &str = "keybindings.json";
/// Tab title of the generated, read-only list of defaults.
pub const DEFAULTS_TITLE: &str = "Default Key Bindings";

// ---- what a key does ---------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Target {
    /// A window command.
    Cmd(Cmd),
    /// An editor command by its internal id (see `EDITOR_COMMANDS`).
    Editor(&'static str),
    /// A view command by its menu action name (folding, scrolling).
    View(&'static str),
    /// Fold everything deeper than this level.
    FoldLevel(u8),
    /// Select the pane's nth tab (0-based; 8 is always the last).
    Tab(u8),
    /// Play a saved macro (see `app::macro_files`), by name.
    MacroFile(&'static str),
}

/// View commands: (menu action name, public id, palette name).
const VIEW_COMMANDS: &[(&str, &str, &str)] = &[
    ("fold", "fold", "Code Folding: Fold"),
    ("unfold", "unfold", "Code Folding: Unfold"),
    ("fold_all", "fold_all", "Code Folding: Fold All"),
    ("unfold_all", "unfold_all", "Code Folding: Unfold All"),
    ("scroll_sel", "show_at_center", "View: Scroll to Selection"),
    ("scroll_up", "scroll_line_up", "View: Scroll Line Up"),
    ("scroll_down", "scroll_line_down", "View: Scroll Line Down"),
];

/// Editor commands whose public id differs from the internal one. The
/// public id is Sublime's command name wherever there is one.
const EDITOR_IDS: &[(&str, &str)] = &[
    ("outdent", "unindent"),
    ("duplicate", "duplicate_line"),
    ("move_up", "swap_line_up"),
    ("move_down", "swap_line_down"),
    ("join", "join_lines"),
    ("line_after", "insert_line_after"),
    ("line_before", "insert_line_before"),
    ("upper", "upper_case"),
    ("lower", "lower_case"),
    ("sort", "sort_lines"),
    ("sort_case", "sort_lines_case_sensitive"),
    ("reverse", "reverse_lines"),
    ("unique", "unique_lines"),
    ("shuffle", "shuffle_lines"),
    ("trim", "trim_trailing_whitespace"),
    ("expand_line", "expand_selection_to_line"),
    ("expand_line_up", "expand_selection_to_line_upward"),
    ("expand_block", "expand_selection_to_block"),
    ("expand_brackets", "expand_selection_to_brackets"),
    ("expand_word", "expand_selection_to_word"),
    ("expand_paragraph", "expand_selection_to_paragraph"),
    ("expand_indentation", "expand_selection_to_indentation"),
    ("split_lines", "split_selection_into_lines"),
    ("cursor_above", "select_lines_up"),
    ("cursor_below", "select_lines_down"),
    ("next_occurrence", "find_under_expand"),
    ("all_occurrences", "find_all_under"),
    ("jump_bracket", "move_to_brackets"),
    ("lower_camel", "lower_camel_case"),
    ("upper_camel", "upper_camel_case"),
    ("snake", "snake_case"),
    ("kebab", "kebab_case"),
    ("sel_sort", "sort_selections"),
    ("sel_sort_ci", "sort_selections_case_insensitive"),
    ("sel_reverse", "reverse_selections"),
    ("sel_unique", "unique_selections"),
    ("sel_shuffle", "shuffle_selections"),
    ("swap_mark", "swap_with_mark"),
    ("tx_json_format", "format_json"),
    ("tx_json_minify", "minify_json"),
    ("tx_json_sort", "sort_json_keys"),
    ("tx_b64_encode", "base64_encode"),
    ("tx_b64_decode", "base64_decode"),
    ("tx_url_encode", "url_encode"),
    ("tx_url_decode", "url_decode"),
    ("tx_html_escape", "html_escape"),
    ("tx_html_unescape", "html_unescape"),
    ("tx_uuid", "insert_uuid"),
    ("tx_date", "insert_date"),
    ("delete_to_end", "delete_to_line_end"),
    ("delete_to_start", "delete_to_line_start"),
    ("wrap_paragraph", "wrap_lines"),
    ("to_spaces", "expand_tabs"),
    ("to_tabs", "unexpand_tabs"),
    ("guess_indent", "detect_indentation"),
    ("bookmark_toggle", "toggle_bookmark"),
    ("bookmark_next", "next_bookmark"),
    ("bookmark_prev", "prev_bookmark"),
    ("bookmark_clear", "clear_bookmarks"),
    ("bookmark_select", "select_all_bookmarks"),
];

/// Sublime command names (taking no arguments) that map to a Strata command
/// under a different id. Names that need arguments are handled in `resolve`.
const SUBLIME_NAMES: &[(&str, Target)] = &[
    ("prompt_open_file", Target::Cmd(Cmd::Open)),
    ("prompt_open", Target::Cmd(Cmd::Open)),
    ("prompt_open_folder", Target::Cmd(Cmd::OpenFolder)),
    ("prompt_save_as", Target::Cmd(Cmd::SaveAs)),
    ("close", Target::Cmd(Cmd::CloseTab)),
    ("close_folder_list", Target::Cmd(Cmd::CloseFolder)),
    ("prompt_open_project_or_workspace", Target::Cmd(Cmd::OpenProject)),
    ("prompt_select_workspace", Target::Cmd(Cmd::SwitchProject)),
    ("close_workspace", Target::Cmd(Cmd::CloseProject)),
    ("save_project_and_workspace_as", Target::Cmd(Cmd::SaveProjectAs)),
    ("close_pane", Target::Cmd(Cmd::CloseGroup)),
    ("redo_or_repeat", Target::Editor("redo")),
];

impl Target {
    /// Runs on the focused editor (and only when an editor has the keyboard).
    pub fn needs_editor(self) -> bool {
        !matches!(self, Target::Cmd(_) | Target::Tab(_))
    }

    /// (id, args) as written in keybindings.json.
    pub fn id(self) -> (&'static str, Option<Value>) {
        match self {
            Target::Cmd(c @ (Cmd::FocusGroup(g) | Cmd::MoveToGroup(g))) => (c.id(), Some(serde_json::json!({ "group": g }))),
            Target::Cmd(c) => (c.id(), None),
            Target::Editor(id) => (editor_public_id(id), None),
            Target::View(name) => (VIEW_COMMANDS.iter().find(|v| v.0 == name).map_or(name, |v| v.1), None),
            Target::FoldLevel(n) => ("fold_by_level", Some(serde_json::json!({ "level": n }))),
            Target::Tab(n) => ("select_by_index", Some(serde_json::json!({ "index": n }))),
            Target::MacroFile(name) => ("run_macro_file", Some(serde_json::json!({ "file": name }))),
        }
    }

    /// What it does, in palette words.
    pub fn name(self) -> String {
        match self {
            Target::Cmd(c) => c.name().to_owned(),
            Target::Editor(id) => EDITOR_COMMANDS.iter().find(|c| c.id == id).map_or(id, |c| c.name).to_owned(),
            Target::View(name) => VIEW_COMMANDS.iter().find(|v| v.0 == name).map_or(name, |v| v.2).to_owned(),
            Target::FoldLevel(n) => format!("Code Folding: Fold Level {n}"),
            Target::Tab(8) => "View: Select Last Tab".to_owned(),
            Target::Tab(n) => format!("View: Select Tab {}", n + 1),
            Target::MacroFile(name) => format!("Tools: Run Macro “{name}”"),
        }
    }

    /// The id with its args, for messages: `focus_group {"group":1}`.
    fn describe(self) -> String {
        match self.id() {
            (id, Some(args)) => format!("{id} {args}"),
            (id, None) => id.to_owned(),
        }
    }
}

fn editor_public_id(internal: &'static str) -> &'static str {
    EDITOR_IDS.iter().find(|e| e.0 == internal).map_or(internal, |e| e.1)
}

/// An editor command by public or internal id.
fn editor_target(name: &str) -> Option<Target> {
    if let Some(&(internal, _)) = EDITOR_IDS.iter().find(|e| e.1 == name) {
        return Some(Target::Editor(internal));
    }
    EDITOR_COMMANDS.iter().find(|c| c.id == name).map(|c| Target::Editor(c.id))
}

// ---- keys ----------------------------------------------------------------------

/// One key press with exactly these modifiers (`Modifiers::COMMAND` = primary).
#[derive(Clone, Copy, Debug)]
pub struct Stroke {
    pub mods: Modifiers,
    pub key: Key,
}

impl PartialEq for Stroke {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key && keys::same_mods(self.mods, other.mods)
    }
}

impl Stroke {
    pub fn label(self) -> String {
        keys::label(self.mods, self.key)
    }

    /// Sublime syntax for this platform: "super+shift+p" on macOS, "ctrl+shift+p" elsewhere.
    pub fn to_sublime(self) -> String {
        let (alt, shift, cmd, ctrl) = keys::parts(self.mods);
        let mut parts: Vec<String> = Vec::new();
        if MAC && cmd {
            parts.push("super".into());
        }
        if ctrl {
            parts.push("ctrl".into());
        }
        if alt {
            parts.push("alt".into());
        }
        if shift {
            parts.push("shift".into());
        }
        parts.push(key_name(self.key));
        parts.join("+")
    }

    /// Does a pressed key match? `logical` compares the typed key, otherwise
    /// the physical one (Shift+1 types "!", Alt+1 types "¡" on a Mac).
    fn hit(self, key: Key, physical: Option<Key>, mods: Modifiers, logical: bool) -> bool {
        keys::exact(mods, self.mods) && if logical { key == self.key } else { physical == Some(self.key) }
    }
}

fn key_name(k: Key) -> String {
    let s = match k {
        Key::ArrowUp => "up",
        Key::ArrowDown => "down",
        Key::ArrowLeft => "left",
        Key::ArrowRight => "right",
        Key::Enter => "enter",
        Key::Tab => "tab",
        Key::Escape => "escape",
        Key::Space => "space",
        Key::Backspace => "backspace",
        Key::Delete => "delete",
        Key::Insert => "insert",
        Key::Home => "home",
        Key::End => "end",
        Key::PageUp => "pageup",
        Key::PageDown => "pagedown",
        Key::Minus => "minus",
        Key::Equals => "equals",
        Key::Plus => "plus",
        Key::Backslash => "\\",
        Key::Slash => "/",
        Key::OpenBracket => "[",
        Key::CloseBracket => "]",
        Key::OpenCurlyBracket => "{",
        Key::CloseCurlyBracket => "}",
        Key::Backtick => "`",
        Key::Comma => ",",
        Key::Period => ".",
        Key::Semicolon => ";",
        Key::Colon => ":",
        Key::Quote => "'",
        Key::Pipe => "|",
        Key::Questionmark => "?",
        Key::Exclamationmark => "!",
        k => return k.name().to_lowercase(),
    };
    s.to_owned()
}

/// "ctrl+shift+k", "super+alt+[", "primary+plus", "f12", "ctrl++".
pub fn parse_stroke(text: &str) -> Result<Stroke, String> {
    let lower = text.trim().to_lowercase();
    if lower.is_empty() {
        return Err("empty key".into());
    }
    // A literal plus is the last part: "ctrl++" or "+".
    let (mods_part, key_part) = if lower == "+" {
        ("", "+")
    } else if let Some(m) = lower.strip_suffix("++") {
        (m, "+")
    } else {
        match lower.rsplit_once('+') {
            Some((m, k)) => (m, k),
            None => ("", lower.as_str()),
        }
    };
    let mut mods = Modifiers::NONE;
    for m in mods_part.split('+').filter(|m| !m.is_empty()) {
        mods = mods.plus(match m.trim() {
            "primary" => Modifiers::COMMAND,
            "super" | "cmd" | "command" | "meta" if MAC => Modifiers::MAC_CMD,
            "super" | "cmd" | "command" | "meta" => {
                return Err(format!("\"{m}\" (the ⌘ key) only exists on macOS: use \"ctrl\" or \"primary\" in \"{text}\""));
            }
            "ctrl" | "control" => Modifiers::CTRL,
            "alt" | "option" | "opt" => Modifiers::ALT,
            "shift" => Modifiers::SHIFT,
            other => return Err(format!("unknown modifier \"{other}\" in \"{text}\"")),
        });
    }
    let key = parse_key(key_part.trim()).ok_or_else(|| format!("unknown key \"{key_part}\" in \"{text}\""))?;
    Ok(Stroke { mods, key })
}

fn parse_key(k: &str) -> Option<Key> {
    let named = match k {
        "up" => Key::ArrowUp,
        "down" => Key::ArrowDown,
        "left" => Key::ArrowLeft,
        "right" => Key::ArrowRight,
        "enter" | "return" | "keypad_enter" => Key::Enter,
        "tab" => Key::Tab,
        "escape" | "esc" => Key::Escape,
        "space" => Key::Space,
        "backspace" => Key::Backspace,
        "delete" | "forward_delete" | "del" => Key::Delete,
        "insert" | "ins" => Key::Insert,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" | "page_up" => Key::PageUp,
        "pagedown" | "page_down" => Key::PageDown,
        "plus" | "keypad_plus" => Key::Plus,
        "minus" | "keypad_minus" => Key::Minus,
        "equals" | "equal" => Key::Equals,
        "keypad_period" | "keypad_decimal" => Key::Period,
        "keypad_divide" => Key::Slash,
        "comma" => Key::Comma,
        "period" => Key::Period,
        "slash" => Key::Slash,
        "backslash" => Key::Backslash,
        "semicolon" => Key::Semicolon,
        "backquote" | "backtick" | "grave" => Key::Backtick,
        "quote" => Key::Quote,
        "open_bracket" => Key::OpenBracket,
        "close_bracket" => Key::CloseBracket,
        _ => {
            if let Some(n) = k.strip_prefix("keypad") {
                return parse_key(n).filter(|_| n.len() == 1 && n.as_bytes()[0].is_ascii_digit());
            }
            if let Some(n) = k.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
                return (1..=35).contains(&n).then(|| Key::from_name(&format!("F{n}"))).flatten();
            }
            // Single characters (letters, digits, punctuation) by egui's own names.
            if k.chars().count() == 1 {
                return Key::from_name(k).or_else(|| Key::from_name(&k.to_uppercase()));
            }
            return None;
        }
    };
    Some(named)
}

// ---- context -------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
pub enum CtxKey {
    /// A terminal tab has the keyboard.
    TerminalFocus,
    /// A panel (the terminal, the find bar) has the keyboard.
    PanelHasFocus,
    /// A panel is open.
    PanelVisible,
    /// The command palette (or another quick panel) is open.
    OverlayVisible,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
pub struct Cond {
    pub key: CtxKey,
    pub want: bool,
}

/// What has the keyboard this frame, for bindings' `context`.
#[derive(Clone, Copy, Default, Debug)]
pub struct KeyContext {
    /// The current tab's editor has keyboard focus.
    pub editor: bool,
    pub terminal: bool,
    pub panel_focus: bool,
    pub panel_visible: bool,
    pub overlay: bool,
}

impl KeyContext {
    fn get(&self, k: CtxKey) -> bool {
        match k {
            CtxKey::TerminalFocus => self.terminal,
            CtxKey::PanelHasFocus => self.panel_focus,
            CtxKey::PanelVisible => self.panel_visible,
            CtxKey::OverlayVisible => self.overlay,
        }
    }
}

fn parse_context(v: &Value) -> Result<Vec<Cond>, String> {
    let list = v.as_array().ok_or("\"context\" should be a list like [{\"key\": \"terminal_focus\"}]")?;
    let mut out = Vec::new();
    for c in list {
        let key = c.get("key").and_then(Value::as_str).ok_or("each context needs a \"key\"")?;
        let k = match key {
            "terminal_focus" => CtxKey::TerminalFocus,
            "panel_has_focus" => CtxKey::PanelHasFocus,
            "panel_visible" => CtxKey::PanelVisible,
            "overlay_visible" | "overlay_has_focus" => CtxKey::OverlayVisible,
            other => return Err(format!("context \"{other}\" isn't supported (terminal_focus, panel_has_focus, panel_visible, overlay_visible are), so the binding is skipped")),
        };
        let operand = match c.get("operand") {
            None => true,
            Some(Value::Bool(b)) => *b,
            Some(other) => return Err(format!("context \"{key}\" takes true or false, not {other}")),
        };
        let want = match c.get("operator").and_then(Value::as_str).unwrap_or("equal") {
            "equal" => operand,
            "not_equal" => !operand,
            op => return Err(format!("context operator \"{op}\" isn't supported (equal, not_equal are)")),
        };
        out.push(Cond { key: k, want });
    }
    out.sort();
    out.dedup();
    Ok(out)
}

// ---- commands by name ----------------------------------------------------------

/// What a keybindings.json entry asks for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum What {
    /// `""` / `"unbound"`: remove what the keys did.
    Unbind,
    /// Swallow the keys and do nothing.
    Noop,
    Run(Target),
}

/// A saved macro's name from `run_macro_file`'s `file`: a bare name, a file
/// name, or a Sublime-style path whose last part is the name (so it always
/// names a file in the Macros folder). None for something that isn't a name.
pub fn saved_macro_name(file: &str) -> Option<&str> {
    let last = file.rsplit(['/', '\\']).next()?;
    let name = last.strip_suffix(".strata-macro").or_else(|| last.strip_suffix(".sublime-macro")).unwrap_or(last).trim();
    (!name.is_empty() && name != "." && name != "..").then_some(name)
}

/// One `&'static` copy of each macro name keybindings.json mentions
/// (targets are `Copy`; the names are few and live as long as the keymap).
pub fn intern(name: &str) -> &'static str {
    static NAMES: std::sync::Mutex<Vec<&'static str>> = std::sync::Mutex::new(Vec::new());
    let mut names = NAMES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(n) = names.iter().find(|n| **n == name) {
        return n;
    }
    let n: &'static str = Box::leak(name.to_owned().into_boxed_str());
    names.push(n);
    n
}

fn arg_str<'a>(args: &'a Map<String, Value>, k: &str) -> Option<&'a str> {
    args.get(k).and_then(Value::as_str)
}

fn arg_bool(args: &Map<String, Value>, k: &str, default: bool) -> bool {
    args.get(k).and_then(Value::as_bool).unwrap_or(default)
}

fn arg_index(args: &Map<String, Value>, k: &str, below: u64, cmd: &str) -> Result<u8, String> {
    match args.get(k).and_then(Value::as_u64) {
        Some(n) if n < below => Ok(n as u8),
        Some(n) => Err(format!("{cmd}: \"{k}\" {n} is out of range (0–{})", below - 1)),
        None => Err(format!("{cmd} needs \"args\": {{\"{k}\": n}}")),
    }
}

/// A command name plus args, Strata's or Sublime's, to what it does.
pub fn resolve(command: &str, args: &Map<String, Value>) -> Result<What, String> {
    let cmd = command.trim();
    let run = |c: Cmd| Ok(What::Run(Target::Cmd(c)));
    let ed = |id: &'static str| Ok(What::Run(Target::Editor(id)));
    match cmd {
        "" | "unbound" => return Ok(What::Unbind),
        "noop" => return Ok(What::Noop),
        "focus_group" => return run(Cmd::FocusGroup(arg_index(args, "group", 4, cmd)?)),
        "move_to_group" => return run(Cmd::MoveToGroup(arg_index(args, "group", 4, cmd)?)),
        "select_by_index" | "select_tab" => return Ok(What::Run(Target::Tab(arg_index(args, "index", 9, cmd)?))),
        "fold_by_level" => {
            let level = arg_index(args, "level", 10, cmd)?.max(1);
            return Ok(What::Run(if level == 1 { Target::View("fold_all") } else { Target::FoldLevel(level) }));
        }
        "scroll_lines" => {
            let amount = args.get("amount").and_then(Value::as_f64).unwrap_or(1.0);
            return Ok(What::Run(Target::View(if amount >= 0.0 { "scroll_up" } else { "scroll_down" })));
        }
        "show_overlay" => {
            return match arg_str(args, "overlay") {
                Some("command_palette") => run(Cmd::CommandPalette),
                Some("goto") => match arg_str(args, "text") {
                    Some(":") => run(Cmd::GotoLine),
                    Some("@") => run(Cmd::GotoSymbol),
                    _ => run(Cmd::GotoAnything),
                },
                other => Err(format!("show_overlay: overlay {other:?} isn't supported (command_palette, goto are)")),
            };
        }
        "show_panel" => {
            return match arg_str(args, "panel") {
                Some("find") => run(Cmd::Find),
                Some("replace") => run(Cmd::Replace),
                Some("find_in_files") => run(Cmd::FindInFiles),
                Some("incremental_find") => run(Cmd::IncrementalFind),
                Some("output.exec") => run(Cmd::ToggleBuildResults),
                other => Err(format!("show_panel: panel {other:?} isn't supported (find, replace, find_in_files, incremental_find, output.exec are)")),
            };
        }
        "toggle_comment" => return ed(if arg_bool(args, "block", false) { "toggle_block_comment" } else { "toggle_comment" }),
        "sort_lines" => return ed(if arg_bool(args, "case_sensitive", false) { "sort_case" } else { "sort" }),
        "select_lines" => return ed(if arg_bool(args, "forward", true) { "cursor_below" } else { "cursor_above" }),
        "delete_word" => return ed(if arg_bool(args, "forward", true) { "delete_word_forward" } else { "delete_word_backward" }),
        "expand_selection" => {
            return match arg_str(args, "to") {
                None | Some("smart") | Some("scope") => ed("expand_selection"),
                Some("line") => ed("expand_line"),
                Some("brackets") => ed("expand_brackets"),
                Some("word") => ed("expand_word"),
                Some("paragraph") => ed("expand_paragraph"),
                Some("indentation") => ed("expand_indentation"),
                Some("block") => ed("expand_block"),
                Some(other) => Err(format!("expand_selection: \"to\": \"{other}\" isn't supported")),
            };
        }
        "permute_lines" => {
            return match arg_str(args, "operation") {
                Some("reverse") => ed("reverse"),
                Some("unique") => ed("unique"),
                Some("shuffle") => ed("shuffle"),
                other => Err(format!("permute_lines: operation {other:?} isn't supported (reverse, unique, shuffle are)")),
            };
        }
        "permute_selection" => {
            return match arg_str(args, "operation") {
                Some("sort") => ed("sel_sort"),
                Some("sort_case_insensitive") => ed("sel_sort_ci"),
                Some("reverse") => ed("sel_reverse"),
                Some("unique") => ed("sel_unique"),
                Some("shuffle") => ed("sel_shuffle"),
                other => Err(format!("permute_selection: operation {other:?} isn't supported")),
            };
        }
        "run_macro_file" => {
            let file = arg_str(args, "file").unwrap_or_default();
            let name = file.rsplit('/').next().unwrap_or_default().trim_end_matches(".sublime-macro");
            return match name {
                "Delete Line" => ed("delete_line"),
                "Add Line" => ed("line_after"),
                "Add Line Before" => ed("line_before"),
                "Delete to Hard EOL" => ed("delete_to_end"),
                "Delete to Hard BOL" | "Delete to BOL" => ed("delete_to_start"),
                // Anything else is a macro saved with Tools ▸ Save Macro….
                _ => match saved_macro_name(file) {
                    Some(name) => Ok(What::Run(Target::MacroFile(intern(name)))),
                    None => Err(format!("run_macro_file: \"{file}\" isn't a macro name")),
                },
            };
        }
        "move_to" => {
            return match arg_str(args, "to") {
                Some("brackets") => ed("jump_bracket"),
                other => Err(format!("move_to: \"to\": {other:?} isn't supported (brackets is)")),
            };
        }
        "clear_bookmarks" if arg_str(args, "name") == Some("mark") => return ed("clear_mark"),
        "new_pane" => return run(if arg_bool(args, "move", true) { Cmd::MoveToNewGroup } else { Cmd::NewGroup }),
        "focus_neighboring_group" => return run(if arg_bool(args, "forward", true) { Cmd::FocusNextGroup } else { Cmd::FocusPrevGroup }),
        "move_to_neighboring_group" => return run(if arg_bool(args, "forward", true) { Cmd::MoveToNextGroup } else { Cmd::MoveToPrevGroup }),
        "build" if arg_bool(args, "select", false) => return run(Cmd::BuildWith),
        "edit_settings" if arg_str(args, "base_file").is_some_and(|f| f.contains("sublime-keymap")) => return run(Cmd::KeyBindings),
        "toggle_setting" => {
            return match arg_str(args, "setting") {
                Some("word_wrap") => run(Cmd::ToggleWordWrap),
                Some("spell_check") => run(Cmd::ToggleSpellCheck),
                other => Err(format!("toggle_setting: setting {other:?} isn't supported (word_wrap, spell_check are)")),
            };
        }
        "set_layout" => {
            let count = |k: &str| args.get(k).and_then(Value::as_array).map_or(0, Vec::len);
            return match (count("cols"), count("rows")) {
                (2, 2) => run(Cmd::LayoutSingle),
                (3, 2) => run(Cmd::LayoutColumns2),
                (4, 2) => run(Cmd::LayoutColumns3),
                (5, 2) => run(Cmd::LayoutColumns4),
                (2, 3) => run(Cmd::LayoutRows2),
                (2, 4) => run(Cmd::LayoutRows3),
                (3, 3) => run(Cmd::LayoutGrid),
                _ => Err("set_layout: only the standard layouts (single, columns 2–4, rows 2–3, grid 4) are supported".into()),
            };
        }
        _ => {}
    }
    if let Some(&c) = Cmd::ALL.iter().find(|c| c.id() == cmd) {
        return run(c);
    }
    if let Some(t) = editor_target(cmd) {
        return Ok(What::Run(t));
    }
    if let Some(v) = VIEW_COMMANDS.iter().find(|v| v.1 == cmd || v.0 == cmd) {
        return Ok(What::Run(Target::View(v.0)));
    }
    if let Some(&(_, t)) = SUBLIME_NAMES.iter().find(|s| s.0 == cmd) {
        return Ok(What::Run(t));
    }
    Err(format!("unknown command \"{cmd}\""))
}

// ---- the keymap ----------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Binding {
    pub keys: Vec<Stroke>,
    /// None: "noop", the keys are swallowed.
    pub action: Option<Target>,
    pub conds: Vec<Cond>,
    /// Line in keybindings.json; None for a built-in binding.
    pub line: Option<usize>,
}

impl Binding {
    fn label(&self) -> String {
        self.keys.iter().map(|s| s.label()).collect::<Vec<_>>().join(" ")
    }

    fn origin(&self) -> String {
        match self.line {
            Some(l) => format!("line {l}"),
            None => "default".into(),
        }
    }

    fn what(&self) -> String {
        self.action.map_or("noop".into(), Target::describe)
    }

    /// Same keys, or one is the start of the other's chord: only one can fire.
    fn overlaps(&self, other: &Binding) -> bool {
        self.conds == other.conds && self.keys.iter().zip(&other.keys).all(|(a, b)| a == b)
    }
}

/// One entry of keybindings.json, parsed.
#[derive(Clone, Debug)]
pub struct Entry {
    pub line: usize,
    pub keys: Vec<Stroke>,
    pub what: What,
    pub conds: Vec<Cond>,
}

/// A chord's first key, waiting for the second.
#[derive(Default)]
pub struct Chord {
    pending: Option<(Stroke, Event)>,
}

impl Chord {
    #[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
    pub fn pending(&self) -> bool {
        self.pending.is_some()
    }
}

/// A binding that fired: its action and the key events it used up.
pub struct Fired {
    pub action: Option<Target>,
    pub events: Vec<Event>,
}

#[derive(Clone, Debug)]
pub struct Keymap {
    bindings: Vec<Binding>,
}

impl Keymap {
    /// Strata's built-in bindings.
    pub fn defaults() -> Self {
        let mut b = Vec::new();
        let mut add = |keys: Vec<Stroke>, t: Target| b.push(Binding { keys, action: Some(t), conds: Vec::new(), line: None });
        let one = |(mods, key): (Modifiers, Key)| vec![Stroke { mods, key }];
        for &c in Cmd::ALL {
            for &k in c.default_keys() {
                add(one(k), Target::Cmd(c));
            }
        }
        for c in EDITOR_COMMANDS {
            for &k in c.keys {
                add(one(k), Target::Editor(c.id));
            }
            if let Some([a, z]) = c.chord {
                add(vec![Stroke { mods: a.0, key: a.1 }, Stroke { mods: z.0, key: z.1 }], Target::Editor(c.id));
            }
        }
        let k_then = |key: Key| vec![Stroke { mods: CMD, key: Key::K }, Stroke { mods: CMD, key }];
        let (fold, scroll) = if MAC { (CMD_ALT, CTRL_ALT) } else { (CTRL_SHIFT, CTRL) };
        add(one((fold, Key::OpenBracket)), Target::View("fold"));
        add(one((fold, Key::CloseBracket)), Target::View("unfold"));
        // Language server commands.
        add(k_then(Key::I), Target::Cmd(Cmd::LspHover));
        add(k_then(Key::R), Target::Cmd(Cmd::LspRename));
        add(k_then(Key::F), Target::Cmd(Cmd::LspFormat));
        add(k_then(Key::M), Target::Cmd(Cmd::LspProblems));
        add(k_then(Key::N), Target::Cmd(Cmd::LspNextProblem));
        add(k_then(Key::P), Target::Cmd(Cmd::LspPrevProblem));
        // Sublime's side bar chord, alongside Cmd+\.
        add(k_then(Key::B), Target::Cmd(Cmd::ToggleSidebar));
        add(k_then(Key::Num1), Target::View("fold_all"));
        add(k_then(Key::J), Target::View("unfold_all"));
        add(k_then(Key::Num0), Target::View("unfold_all"));
        let digits = [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9];
        for (n, &d) in digits.iter().enumerate().skip(1) {
            add(k_then(d), Target::FoldLevel(n as u8 + 1));
        }
        if MAC {
            add(one((CTRL, Key::L)), Target::View("scroll_sel"));
        }
        add(one((scroll, Key::ArrowUp)), Target::View("scroll_up"));
        add(one((scroll, Key::ArrowDown)), Target::View("scroll_down"));
        // Cmd+1…9 picks a tab (Alt on Windows/Linux, like Sublime); 9 is always the last.
        for (n, &d) in digits.iter().enumerate() {
            add(one((if MAC { CMD } else { ALT }, d)), Target::Tab(n as u8));
        }
        Self { bindings: b }
    }

    /// The defaults with a keybindings.json applied. A file that isn't valid
    /// JSON is an error (keep the old keymap); bad entries are skipped and
    /// reported, as are user bindings that shadow others.
    pub fn with_user(text: &str) -> Result<(Self, Vec<String>), String> {
        let (entries, mut warnings) = parse_file(text)?;
        let mut keymap = Self::defaults();
        keymap.apply(entries, &mut warnings);
        // In file order (each starts "line N:").
        warnings.sort_by_key(|w| w.strip_prefix("line ").and_then(|r| r.split(':').next()).and_then(|n| n.parse::<usize>().ok()).unwrap_or(0));
        Ok((keymap, warnings))
    }

    fn apply(&mut self, entries: Vec<Entry>, warnings: &mut Vec<String>) {
        // Binding a command replaces its built-in keys (in the same context).
        let rebound: Vec<(Target, Vec<Cond>)> = entries
            .iter()
            .filter_map(|e| match e.what {
                What::Run(t) => Some((t, e.conds.clone())),
                _ => None,
            })
            .collect();
        self.bindings.retain(|b| b.line.is_some() || !rebound.iter().any(|(t, c)| b.action == Some(*t) && b.conds == *c));
        for e in entries {
            let new = Binding { keys: e.keys, action: None, conds: e.conds, line: Some(e.line) };
            let (gone, kept): (Vec<Binding>, Vec<Binding>) = std::mem::take(&mut self.bindings).into_iter().partition(|b| b.overlaps(&new));
            self.bindings = kept;
            let action = match e.what {
                What::Unbind => {
                    if gone.is_empty() {
                        warnings.push(format!("line {}: nothing was bound to {} to unbind", e.line, new.label()));
                    }
                    continue;
                }
                What::Noop => None,
                What::Run(t) => Some(t),
            };
            let now = action.map_or("noop".into(), Target::describe);
            for old in gone.iter().filter(|old| old.action != action || old.keys.len() != new.keys.len()) {
                warnings.push(if old.keys.len() == new.keys.len() {
                    format!("line {}: {} now runs {now} instead of {} ({})", e.line, new.label(), old.what(), old.origin())
                } else {
                    format!("line {}: {} now runs {now}, which turns off {} on {} ({})", e.line, new.label(), old.what(), old.label(), old.origin())
                });
            }
            self.bindings.push(Binding { action, ..new });
        }
    }

    #[cfg(test)]
    pub fn bindings(&self) -> &[Binding] {
        &self.bindings
    }

    /// Context-free bindings for `t`, in order (the first is what menus show).
    fn for_target(&self, t: Target) -> impl Iterator<Item = &Binding> {
        self.bindings.iter().filter(move |b| b.action == Some(t) && b.conds.is_empty())
    }

    /// Single-key shortcuts for `t`.
    #[cfg(test)]
    pub fn strokes(&self, t: Target) -> Vec<(Modifiers, Key)> {
        self.for_target(t).filter(|b| b.keys.len() == 1).map(|b| (b.keys[0].mods, b.keys[0].key)).collect()
    }

    /// Shortcut text for `t`: "⇧ ⌘ P", or a chord "⌘ K ⌘ U".
    pub fn label(&self, t: Target) -> Option<String> {
        self.for_target(t).next().map(Binding::label)
    }

    /// Every context-free shortcut for `t` ("⇧ ⌘ P", "⌘ K ⌘ U"), first one first.
    pub fn labels(&self, t: Target) -> Vec<String> {
        self.for_target(t).map(Binding::label).collect()
    }

    /// Bindings limited to a context (keys, what they run, the context), in file order.
    pub fn context_bindings(&self) -> Vec<(String, String, String)> {
        let name = |k: CtxKey| match k {
            CtxKey::TerminalFocus => "terminal_focus",
            CtxKey::PanelHasFocus => "panel_has_focus",
            CtxKey::PanelVisible => "panel_visible",
            CtxKey::OverlayVisible => "overlay_visible",
        };
        self.bindings
            .iter()
            .filter(|b| !b.conds.is_empty())
            .map(|b| {
                let what = b.action.map_or("noop".to_owned(), Target::name);
                let ctx = b.conds.iter().map(|c| if c.want { name(c.key).to_owned() } else { format!("not {}", name(c.key)) }).collect::<Vec<_>>().join(", ");
                (b.label(), what, ctx)
            })
            .collect()
    }

    /// For a menu item: an accelerator for a single key, or display text for a chord.
    pub fn menu_keys(&self, t: Target) -> (Option<(Modifiers, Key)>, Option<String>) {
        match self.for_target(t).next() {
            Some(b) if b.keys.len() == 1 => (Some((b.keys[0].mods, b.keys[0].key)), None),
            Some(b) => (None, Some(b.label())),
            None => (None, None),
        }
    }

    /// Is anything bound to this key (alone or as a chord's first key)?
    pub fn is_bound(&self, mods: Modifiers, key: Key) -> bool {
        let s = Stroke { mods, key };
        self.bindings.iter().any(|b| b.keys[0] == s)
    }

    fn active(&self, b: &Binding, kc: &KeyContext) -> bool {
        if !b.conds.iter().all(|c| kc.get(c.key) == c.want) {
            return false;
        }
        if b.action.is_some_and(Target::needs_editor) && !kc.editor {
            return false;
        }
        // A focused terminal keeps plain, Ctrl and Alt keys for the shell, unless
        // the binding says it's meant for the terminal.
        if kc.terminal && !b.conds.iter().any(|c| c.key == CtxKey::TerminalFocus) {
            let cmd = match b.action {
                Some(Target::Cmd(c)) => Some(c),
                _ => None,
            };
            return crate::terminal::app_keeps(cmd, b.keys[0].mods);
        }
        true
    }

    /// The binding a key press completes: `first` is a pending chord key.
    /// Later bindings win; the typed key wins over the physical one.
    fn find(&self, first: Option<Stroke>, ev: (Key, Option<Key>, Modifiers), kc: &KeyContext, chord_start: bool) -> Option<&Binding> {
        let (key, physical, mods) = ev;
        let want = if first.is_some() || chord_start { 2 } else { 1 };
        let at = usize::from(first.is_some());
        for logical in [true, false] {
            let hit = self.bindings.iter().rev().find(|b| {
                b.keys.len() == want
                    && first.is_none_or(|f| b.keys[0] == f)
                    && b.keys[at].hit(key, physical, mods, logical)
                    && self.active(b, kc)
            });
            if hit.is_some() {
                return hit;
            }
        }
        None
    }

    /// Would a press of `key` with `mods` run a binding now (alone, as a
    /// chord's first key, or completing the pending chord)?
    pub fn takes(&self, mods: Modifiers, key: Key, kc: &KeyContext, chord: &Chord) -> bool {
        let press = (key, Some(key), mods);
        if let Some((first, _)) = &chord.pending
            && self.find(Some(*first), press, kc, false).is_some()
        {
            return true;
        }
        self.find(None, press, kc, true).is_some() || self.find(None, press, kc, false).is_some()
    }

    /// Take this frame's key presses that are bound, in order. A chord's first
    /// key waits in `chord` for the next press; Escape cancels it.
    pub fn dispatch(&self, ctx: &egui::Context, kc: &KeyContext, chord: &mut Chord) -> Vec<Fired> {
        let mut fired = Vec::new();
        ctx.input_mut(|i| {
            let mut keep = Vec::with_capacity(i.events.len());
            // A bound key's own text ("≈" for Option+X on a Mac) isn't typed.
            let mut took_key = false;
            for ev in std::mem::take(&mut i.events) {
                if std::mem::take(&mut took_key) && matches!(ev, Event::Text(_)) {
                    continue;
                }
                let Event::Key { key, physical_key, pressed: true, modifiers, .. } = ev else {
                    keep.push(ev);
                    continue;
                };
                if is_modifier(key) {
                    keep.push(ev);
                    continue;
                }
                let press = (key, physical_key, modifiers);
                took_key = true;
                if let Some((first, first_ev)) = chord.pending.take() {
                    if key == Key::Escape {
                        continue;
                    }
                    if let Some(b) = self.find(Some(first), press, kc, false) {
                        fired.push(Fired { action: b.action, events: vec![first_ev, ev] });
                        continue;
                    }
                    // Not a chord: the key counts on its own.
                }
                if let Some(b) = self.find(None, press, kc, true) {
                    chord.pending = Some((b.keys[0], ev));
                } else if let Some(b) = self.find(None, press, kc, false) {
                    fired.push(Fired { action: b.action, events: vec![ev] });
                } else {
                    took_key = false;
                    keep.push(ev);
                }
            }
            i.events = keep;
        });
        fired
    }
}

fn is_modifier(k: Key) -> bool {
    matches!(k, Key::ShiftLeft | Key::ShiftRight | Key::ControlLeft | Key::ControlRight | Key::AltLeft | Key::AltRight | Key::SuperLeft | Key::SuperRight)
}

// ---- the file --------------------------------------------------------------------

/// Comments and trailing commas blanked out with spaces, so byte offsets (and
/// so line numbers in errors) match the original text.
fn blank_jsonc(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = b.to_vec();
    let blank = |out: &mut Vec<u8>, from: usize, to: usize| {
        for c in &mut out[from..to] {
            if *c != b'\n' {
                *c = b' ';
            }
        }
    };
    let mut i = 0;
    let mut last_comma: Option<usize> = None;
    while i < b.len() {
        match b[i] {
            b'"' => {
                last_comma = None;
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                i += 1;
            }
            b'/' if b.get(i + 1) == Some(&b'/') => {
                let end = s[i..].find('\n').map_or(b.len(), |n| i + n);
                blank(&mut out, i, end);
                i = end;
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                let end = s[i + 2..].find("*/").map_or(b.len(), |n| i + 2 + n + 2);
                blank(&mut out, i, end);
                i = end;
            }
            b',' => {
                last_comma = Some(i);
                i += 1;
            }
            b'}' | b']' => {
                if let Some(c) = last_comma.take() {
                    out[c] = b' ';
                }
                i += 1;
            }
            c if c.is_ascii_whitespace() => i += 1,
            _ => {
                last_comma = None;
                i += 1;
            }
        }
    }
    // Only ASCII bytes were replaced, by ASCII.
    String::from_utf8(out).unwrap_or_default()
}

/// Line (1-based) where each element of the top-level array starts.
fn element_lines(clean: &str) -> Vec<usize> {
    let b = clean.as_bytes();
    let mut lines = Vec::new();
    let (mut depth, mut line, mut want) = (0usize, 1usize, false);
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'\n' {
            line += 1;
        } else if !c.is_ascii_whitespace() {
            if depth == 1 && want && c != b']' {
                lines.push(line);
                want = false;
            }
            match c {
                b'"' => {
                    i += 1;
                    while i < b.len() && b[i] != b'"' {
                        if b[i] == b'\n' {
                            line += 1;
                        }
                        i += if b[i] == b'\\' { 2 } else { 1 };
                    }
                }
                b'[' | b'{' => {
                    depth += 1;
                    want = depth == 1;
                }
                b']' | b'}' => depth = depth.saturating_sub(1),
                b',' if depth == 1 => want = true,
                _ => {}
            }
        }
        i += 1;
    }
    lines
}

/// Parse keybindings.json. Err only when the file as a whole is unusable.
pub fn parse_file(text: &str) -> Result<(Vec<Entry>, Vec<String>), String> {
    let clean = blank_jsonc(text);
    if clean.trim().is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let value: Value = serde_json::from_str(&clean).map_err(|e| format!("line {}: {}", e.line(), strip_position(&e.to_string())))?;
    let Value::Array(items) = value else {
        return Err("the file should be a list: [ { \"keys\": [\"primary+shift+k\"], \"command\": \"delete_line\" } ]".into());
    };
    let lines = element_lines(&clean);
    let mut entries = Vec::new();
    let mut warnings = Vec::new();
    for (n, item) in items.iter().enumerate() {
        let line = lines.get(n).copied().unwrap_or(1);
        match parse_entry(item, line) {
            Ok(e) => entries.push(e),
            Err(w) => warnings.push(format!("line {line}: {w}")),
        }
    }
    Ok((entries, warnings))
}

fn strip_position(msg: &str) -> &str {
    msg.find(" at line ").map_or(msg, |n| &msg[..n])
}

fn parse_entry(item: &Value, line: usize) -> Result<Entry, String> {
    let obj = item.as_object().ok_or("each binding should be an object: { \"keys\": [...], \"command\": \"...\" }")?;
    let keys = match obj.get("keys") {
        Some(Value::Array(k)) if !k.is_empty() => k,
        Some(Value::String(_)) => return Err("\"keys\" should be a list: [\"primary+shift+k\"]".into()),
        _ => return Err("missing \"keys\"".into()),
    };
    if keys.len() > 2 {
        return Err("chords of more than two keys aren't supported".into());
    }
    let keys = keys
        .iter()
        .map(|k| k.as_str().ok_or_else(|| format!("key {k} should be a string")).and_then(parse_stroke))
        .collect::<Result<Vec<_>, _>>()?;
    let command = match obj.get("command") {
        Some(Value::String(c)) => c.as_str(),
        Some(Value::Null) => "",
        Some(_) => return Err("\"command\" should be a string".into()),
        None => return Err("missing \"command\"".into()),
    };
    let empty = Map::new();
    let args = match obj.get("args") {
        None | Some(Value::Null) => &empty,
        Some(Value::Object(a)) => a,
        Some(_) => return Err("\"args\" should be an object".into()),
    };
    let conds = match obj.get("context") {
        None | Some(Value::Null) => Vec::new(),
        Some(c) => parse_context(c)?,
    };
    let what = resolve(command, args)?;
    Ok(Entry { line, keys, what, conds })
}

/// Every command a key can run: window commands, editor commands, view
/// commands, fold levels and tab digits.
pub fn all_targets() -> Vec<Target> {
    let mut targets: Vec<Target> = Cmd::ALL.iter().map(|&c| Target::Cmd(c)).collect();
    targets.extend(EDITOR_COMMANDS.iter().map(|c| Target::Editor(c.id)));
    targets.extend(VIEW_COMMANDS.iter().map(|v| Target::View(v.0)));
    targets.extend((2..=9).map(Target::FoldLevel));
    targets.extend((0..9).map(Target::Tab));
    targets
}

/// The read-only list of every command and its default keys, as JSON with comments.
pub fn defaults_text() -> String {
    let defaults = Keymap::defaults();
    let platform = if MAC {
        "macOS"
    } else if cfg!(windows) {
        "Windows"
    } else {
        "Linux"
    };
    let mut out = format!(
        "// {DEFAULTS_TITLE} ({platform}). Read-only: these are Strata's built-in keys.\n\
         // Copy an entry into your {FILE_NAME} (the other pane) and change its keys there.\n\
         // \"primary\" works too: ⌘ on macOS, Ctrl elsewhere. Commands without a key are commented out.\n[\n"
    );
    let targets = all_targets();
    let entry = |keys: &str, t: Target| {
        let (id, args) = t.id();
        let args = args.map(|a| format!(", \"args\": {a}")).unwrap_or_default();
        format!("{{ \"keys\": [{keys}], \"command\": \"{id}\"{args} }},")
    };
    for t in targets {
        let bound: Vec<&Binding> = defaults.bindings.iter().filter(|b| b.action == Some(t)).collect();
        if bound.is_empty() {
            out.push_str(&format!("\t// {:<64} // {}\n", entry("", t), t.name()));
        }
        for b in bound {
            let keys: Vec<String> = b.keys.iter().map(|s| serde_json::to_string(&s.to_sublime()).unwrap_or_default()).collect();
            out.push_str(&format!("\t{:<67} // {}\n", entry(&keys.join(", "), t), t.name()));
        }
    }
    out.push_str("]\n");
    out
}

/// What a new keybindings.json starts with.
pub fn template() -> String {
    let palette = if MAC { "super+shift+p" } else { "ctrl+shift+p" };
    format!(
        "// Strata key bindings. Entries here override the defaults (the other pane).\n\
         //\n\
         //   {{ \"keys\": [\"primary+shift+k\"], \"command\": \"delete_line\" }},\n\
         //   {{ \"keys\": [\"primary+k\", \"primary+u\"], \"command\": \"upper_case\" }},   // a chord\n\
         //   {{ \"keys\": [\"{palette}\"], \"command\": \"\" }},   // unbind a key\n\
         //\n\
         // Modifiers: primary (⌘ on macOS, Ctrl elsewhere), super/cmd, ctrl, alt/option, shift.\n\
         // Binding a command replaces its default keys. Later entries win.\n\
         // Common command names from other editors' keymaps work too, with their args, e.g.\n\
         //   {{ \"keys\": [\"{palette}\"], \"command\": \"show_overlay\", \"args\": {{\"overlay\": \"command_palette\"}} }}\n\
         // \"context\": [{{\"key\": \"terminal_focus\"}}] limits a binding to the terminal\n\
         // (also panel_has_focus, panel_visible, overlay_visible).\n\
         // Changes apply when you save; problems show in the status bar.\n\
         [\n\
         \n\
         ]\n"
    )
}

// ---- the keymap in effect ---------------------------------------------------------

thread_local! {
    static CURRENT: RefCell<Option<Rc<Keymap>>> = const { RefCell::new(None) };
}

/// The keymap in effect (all windows run on the UI thread).
pub fn current() -> Rc<Keymap> {
    CURRENT.with(|c| c.borrow_mut().get_or_insert_with(|| Rc::new(Keymap::defaults())).clone())
}

pub fn install(keymap: Keymap) {
    CURRENT.with(|c| *c.borrow_mut() = Some(Rc::new(keymap)));
}

/// keybindings.json on disk, reloaded when it changes.
pub struct KeymapFile {
    path: Option<PathBuf>,
    /// (modified, length) when last read; None if there was no file.
    stamp: Option<(SystemTime, u64)>,
}

impl KeymapFile {
    pub fn new(config: Option<&Path>) -> Self {
        Self { path: config.map(|d| d.join(FILE_NAME)), stamp: None }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    fn disk_stamp(&self) -> Option<(SystemTime, u64)> {
        let m = std::fs::metadata(self.path.as_ref()?).ok()?;
        Some((m.modified().ok()?, m.len()))
    }

    /// Read the file and install its keymap. Returns a status-bar message:
    /// problems, or (when `quiet` is false) that it was reloaded.
    pub fn load(&mut self, quiet: bool) -> Option<String> {
        self.stamp = self.disk_stamp();
        let Some(path) = self.path.as_ref().filter(|_| self.stamp.is_some()) else {
            install(Keymap::defaults());
            return (!quiet).then(|| "Key bindings: using the defaults".into());
        };
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => return Some(format!("Could not read {FILE_NAME}: {e} (keeping the current key bindings)")),
        };
        match Keymap::with_user(&text) {
            Ok((keymap, warnings)) => {
                install(keymap);
                for w in &warnings {
                    log::warn!("{FILE_NAME} {w}");
                }
                match warnings.len() {
                    0 if quiet => None,
                    0 => Some("Key bindings reloaded".into()),
                    1 => Some(format!("{FILE_NAME} {}", warnings[0])),
                    n => Some(format!("{FILE_NAME} {} (+{} more)", warnings[0], n - 1)),
                }
            }
            Err(e) => Some(format!("{FILE_NAME} {e} (keeping the previous key bindings)")),
        }
    }

    /// Reload if the file changed (or appeared, or went away) since the last read.
    pub fn poll(&mut self) -> Option<String> {
        if self.path.is_none() || self.disk_stamp() == self.stamp {
            return None;
        }
        self.load(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(mods: Modifiers, key: Key) -> Stroke {
        Stroke { mods, key }
    }

    fn user(text: &str) -> (Keymap, Vec<String>) {
        Keymap::with_user(text).expect("parses")
    }

    #[test]
    fn parses_modifiers_keys_and_primary() {
        assert_eq!(parse_stroke("ctrl+shift+k").unwrap(), s(CTRL_SHIFT, Key::K));
        assert_eq!(parse_stroke("primary+shift+p").unwrap(), s(CMD_SHIFT, Key::P));
        assert_eq!(parse_stroke("Alt+Option+F12").unwrap(), s(ALT, Key::F12));
        assert_eq!(parse_stroke("ctrl+alt+shift+f24").unwrap(), s(CTRL_ALT.plus(Modifiers::SHIFT), Key::F24));
        assert_eq!(parse_stroke("shift+f3").unwrap(), s(SHIFT, Key::F3));
        assert_eq!(parse_stroke("escape").unwrap(), s(NONE, Key::Escape));
        for (text, key) in [
            ("up", Key::ArrowUp),
            ("down", Key::ArrowDown),
            ("left", Key::ArrowLeft),
            ("right", Key::ArrowRight),
            ("home", Key::Home),
            ("end", Key::End),
            ("pageup", Key::PageUp),
            ("page_down", Key::PageDown),
            ("enter", Key::Enter),
            ("tab", Key::Tab),
            ("space", Key::Space),
            ("backspace", Key::Backspace),
            ("delete", Key::Delete),
            ("keypad5", Key::Num5),
            ("keypad_enter", Key::Enter),
            ("7", Key::Num7),
            ("z", Key::Z),
        ] {
            assert_eq!(parse_stroke(text).unwrap().key, key, "{text}");
        }
        if MAC {
            assert_eq!(parse_stroke("super+k").unwrap(), s(CMD, Key::K), "super is ⌘ (same as primary)");
            assert_eq!(parse_stroke("cmd+alt+down").unwrap(), s(CMD_ALT, Key::ArrowDown));
            assert_ne!(parse_stroke("ctrl+k").unwrap(), s(CMD, Key::K), "ctrl is not ⌘ on macOS");
        } else {
            assert!(parse_stroke("super+k").unwrap_err().contains("macOS"));
            assert_eq!(parse_stroke("ctrl+k").unwrap(), s(CMD, Key::K), "primary is ctrl");
        }
        assert!(parse_stroke("hyper+k").unwrap_err().contains("hyper"));
        assert!(parse_stroke("ctrl+banana").unwrap_err().contains("banana"));
        assert!(parse_stroke("").is_err());
        assert!(parse_stroke("keypad_multiply").is_err());
    }

    #[test]
    fn parses_punctuation() {
        for (text, key) in [
            ("ctrl+/", Key::Slash),
            ("ctrl+\\", Key::Backslash),
            ("ctrl+[", Key::OpenBracket),
            ("ctrl+]", Key::CloseBracket),
            ("ctrl+`", Key::Backtick),
            ("ctrl+,", Key::Comma),
            ("ctrl+.", Key::Period),
            ("ctrl+;", Key::Semicolon),
            ("ctrl+'", Key::Quote),
            ("ctrl+=", Key::Equals),
            ("ctrl+-", Key::Minus),
            ("ctrl+plus", Key::Plus),
            ("ctrl+minus", Key::Minus),
            ("ctrl+equals", Key::Equals),
            ("ctrl++", Key::Plus),
            ("+", Key::Plus),
            ("keypad_plus", Key::Plus),
        ] {
            assert_eq!(parse_stroke(text).unwrap().key, key, "{text}");
        }
        assert_eq!(parse_stroke("ctrl++").unwrap().mods, CTRL);
    }

    #[test]
    fn strokes_round_trip_through_sublime_syntax() {
        for b in Keymap::defaults().bindings() {
            for &k in &b.keys {
                assert_eq!(parse_stroke(&k.to_sublime()).unwrap(), k, "{}", k.to_sublime());
            }
        }
    }

    #[test]
    fn chords_and_comments_parse_with_line_numbers() {
        let text = "// mine\n[\n  /* block\n  comment */\n  { \"keys\": [\"primary+k\", \"primary+t\"], \"command\": \"title_case\" },\n  { \"keys\": [\"ctrl+alt+x\"], \"command\": \"no_such_thing\" },\n]\n";
        let (entries, warnings) = parse_file(text).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].line, 5);
        assert_eq!(entries[0].keys, [s(CMD, Key::K), s(CMD, Key::T)]);
        assert_eq!(entries[0].what, What::Run(Target::Editor("title_case")));
        assert_eq!(warnings, ["line 6: unknown command \"no_such_thing\""]);
        let err = parse_file("[\n  { \"keys\": [\"ctrl+a\"] \"command\": \"save\" }\n]").unwrap_err();
        assert!(err.starts_with("line 2:"), "{err}");
    }

    #[test]
    fn sublime_names_and_args_map_to_strata_commands() {
        let none = Map::new();
        let cases: &[(&str, Value, Target)] = &[
            ("show_overlay", serde_json::json!({"overlay": "command_palette"}), Target::Cmd(Cmd::CommandPalette)),
            ("show_overlay", serde_json::json!({"overlay": "goto", "show_files": true}), Target::Cmd(Cmd::GotoAnything)),
            ("show_overlay", serde_json::json!({"overlay": "goto", "text": ":"}), Target::Cmd(Cmd::GotoLine)),
            ("show_overlay", serde_json::json!({"overlay": "goto", "text": "@"}), Target::Cmd(Cmd::GotoSymbol)),
            ("show_panel", serde_json::json!({"panel": "replace", "reverse": false}), Target::Cmd(Cmd::Replace)),
            ("show_panel", serde_json::json!({"panel": "find_in_files"}), Target::Cmd(Cmd::FindInFiles)),
            ("toggle_comment", serde_json::json!({"block": false}), Target::Editor("toggle_comment")),
            ("toggle_comment", serde_json::json!({"block": true}), Target::Editor("toggle_block_comment")),
            ("expand_selection", serde_json::json!({"to": "line"}), Target::Editor("expand_line")),
            ("expand_selection", serde_json::json!({"to": "brackets"}), Target::Editor("expand_brackets")),
            ("expand_selection", serde_json::json!({"to": "smart"}), Target::Editor("expand_selection")),
            ("select_lines", serde_json::json!({"forward": false}), Target::Editor("cursor_above")),
            ("select_lines", serde_json::json!({"forward": true}), Target::Editor("cursor_below")),
            ("sort_lines", serde_json::json!({"case_sensitive": true}), Target::Editor("sort_case")),
            ("run_macro_file", serde_json::json!({"file": "res://Packages/Default/Delete Line.sublime-macro"}), Target::Editor("delete_line")),
            ("run_macro_file", serde_json::json!({"file": "res://Packages/Default/Add Line Before.sublime-macro"}), Target::Editor("line_before")),
            ("focus_group", serde_json::json!({"group": 1}), Target::Cmd(Cmd::FocusGroup(1))),
            ("move_to_group", serde_json::json!({"group": 3}), Target::Cmd(Cmd::MoveToGroup(3))),
            ("select_by_index", serde_json::json!({"index": 8}), Target::Tab(8)),
            ("fold_by_level", serde_json::json!({"level": 3}), Target::FoldLevel(3)),
            ("set_layout", serde_json::json!({"cols": [0.0, 0.5, 1.0], "rows": [0.0, 1.0], "cells": [[0, 0, 1, 1], [1, 0, 2, 1]]}), Target::Cmd(Cmd::LayoutColumns2)),
            ("toggle_setting", serde_json::json!({"setting": "word_wrap"}), Target::Cmd(Cmd::ToggleWordWrap)),
            ("build", serde_json::json!({"select": true}), Target::Cmd(Cmd::BuildWith)),
            ("edit_settings", serde_json::json!({"base_file": "${packages}/Default/Default ($platform).sublime-keymap"}), Target::Cmd(Cmd::KeyBindings)),
        ];
        for (name, args, want) in cases {
            assert_eq!(resolve(name, args.as_object().unwrap()), Ok(What::Run(*want)), "{name} {args}");
        }
        for (name, want) in [
            ("toggle_side_bar", Target::Cmd(Cmd::ToggleSidebar)),
            ("duplicate_line", Target::Editor("duplicate")),
            ("find_under_expand", Target::Editor("next_occurrence")),
            ("goto_line", Target::Cmd(Cmd::GotoLine)),
            ("save", Target::Cmd(Cmd::Save)),
            ("new_file", Target::Cmd(Cmd::NewTab)),
            ("close", Target::Cmd(Cmd::CloseTab)),
            ("next_view", Target::Cmd(Cmd::NextTab)),
            ("prev_view", Target::Cmd(Cmd::PrevTab)),
            ("swap_line_up", Target::Editor("move_up")),
            ("swap_line_down", Target::Editor("move_down")),
            ("join_lines", Target::Editor("join")),
            ("split_selection_into_lines", Target::Editor("split_lines")),
            ("prompt_open_file", Target::Cmd(Cmd::Open)),
            ("unindent", Target::Editor("outdent")),
            ("duplicate", Target::Editor("duplicate")),
            ("show_at_center", Target::View("scroll_sel")),
        ] {
            assert_eq!(resolve(name, &none), Ok(What::Run(want)), "{name}");
        }
        assert_eq!(resolve("", &none), Ok(What::Unbind));
        assert_eq!(resolve("unbound", &none), Ok(What::Unbind));
        assert_eq!(resolve("noop", &none), Ok(What::Noop));
        assert!(resolve("focus_group", &none).unwrap_err().contains("group"));
        assert!(resolve("show_overlay", serde_json::json!({"overlay": "console"}).as_object().unwrap()).is_err());
    }

    #[test]
    fn every_target_id_resolves_back_to_itself() {
        let defaults = Keymap::defaults();
        let mut targets: Vec<Target> = Cmd::ALL.iter().map(|&c| Target::Cmd(c)).collect();
        targets.extend(EDITOR_COMMANDS.iter().map(|c| Target::Editor(c.id)));
        targets.extend(VIEW_COMMANDS.iter().map(|v| Target::View(v.0)));
        targets.extend(defaults.bindings().iter().filter_map(|b| b.action));
        for t in targets {
            let (id, args) = t.id();
            let args = args.and_then(|a| a.as_object().cloned()).unwrap_or_default();
            assert_eq!(resolve(id, &args), Ok(What::Run(t)), "{id}");
        }
    }

    #[test]
    fn defaults_have_no_conflicts() {
        let b = Keymap::defaults().bindings;
        for (i, x) in b.iter().enumerate() {
            for y in &b[i + 1..] {
                assert!(!x.overlaps(y), "{} is bound to {} and {}", x.label(), x.what(), y.what());
            }
        }
    }

    #[test]
    fn user_binding_replaces_a_commands_defaults_and_warns_on_shadowing() {
        let (k, warnings) = user(r#"[ { "keys": ["ctrl+alt+p"], "command": "command_palette" } ]"#);
        assert_eq!(k.strokes(Target::Cmd(Cmd::CommandPalette)), [(CTRL_ALT, Key::P)]);
        assert!(!k.is_bound(CMD_SHIFT, Key::P), "the old key is free");
        if MAC {
            assert!(warnings.is_empty(), "{warnings:?}");
        } else {
            // Ctrl+Alt+P is Switch Project off macOS.
            assert_eq!(warnings.len(), 1, "{warnings:?}");
            assert!(warnings[0].contains("switch_project"), "{warnings:?}");
            assert!(k.strokes(Target::Cmd(Cmd::SwitchProject)).is_empty());
        }

        // Taking another command's key: it loses that key and you're told.
        let (k, warnings) = user(r#"[ { "keys": ["primary+shift+p"], "command": "goto_line" } ]"#);
        assert_eq!(k.strokes(Target::Cmd(Cmd::GotoLine)), [(CMD_SHIFT, Key::P)]);
        assert!(k.strokes(Target::Cmd(Cmd::CommandPalette)).is_empty());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].starts_with("line 1:") && warnings[0].contains("command_palette (default)"), "{warnings:?}");

        // Two user lines on the same key: the later wins, with a warning.
        let (k, warnings) = user("[\n{ \"keys\": [\"ctrl+alt+j\"], \"command\": \"save\" },\n{ \"keys\": [\"ctrl+alt+j\"], \"command\": \"save_all\" }\n]");
        assert_eq!(k.strokes(Target::Cmd(Cmd::SaveAll)), [(CTRL_ALT, Key::J)]);
        assert!(!k.strokes(Target::Cmd(Cmd::Save)).contains(&(CTRL_ALT, Key::J)));
        assert!(warnings.iter().any(|w| w.starts_with("line 3:") && w.contains("(line 2)")), "{warnings:?}");
    }

    #[test]
    fn unbind_and_noop() {
        let (k, warnings) = user(r#"[ { "keys": ["primary+shift+p"], "command": "" }, { "keys": ["primary+s"], "command": "unbound" } ]"#);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(k.label(Target::Cmd(Cmd::CommandPalette)).is_none());
        assert!(!k.is_bound(CMD, Key::S));
        assert!(!k.strokes(Target::Cmd(Cmd::Save)).contains(&(CMD, Key::S)));
        let (k, _) = user(r#"[ { "keys": ["primary+n"], "command": "noop" } ]"#);
        assert!(k.is_bound(CMD, Key::N));
        assert!(k.label(Target::Cmd(Cmd::NewTab)).is_none());
        let (_, warnings) = user(r#"[ { "keys": ["ctrl+alt+shift+f19"], "command": "" } ]"#);
        assert_eq!(warnings.len(), 1, "unbinding a free key is reported");
    }

    #[test]
    fn a_single_key_and_a_chord_starting_with_it_cannot_both_live() {
        let (k, warnings) = user(r#"[ { "keys": ["primary+k"], "command": "save" } ]"#);
        assert_eq!(k.strokes(Target::Cmd(Cmd::Save)), [(CMD, Key::K)]);
        assert!(k.label(Target::Editor("upper")).is_none(), "the ⌘K chords are gone");
        assert!(warnings.iter().any(|w| w.contains("upper_case")), "{warnings:?}");
        // And a user chord takes over a single key that was its first stroke.
        let (k, _) = user(r#"[ { "keys": ["primary+d", "primary+d"], "command": "duplicate_line" } ]"#);
        assert!(k.strokes(Target::Editor("next_occurrence")).is_empty());
        assert_eq!(k.label(Target::Editor("duplicate")), Some(format!("{} {}", label(CMD, Key::D), label(CMD, Key::D))));
    }

    #[test]
    fn contexts() {
        let (k, warnings) = user(
            r#"[
            { "keys": ["ctrl+alt+t"], "command": "toggle_terminal", "context": [{"key": "terminal_focus", "operator": "equal", "operand": true}] },
            { "keys": ["ctrl+alt+u"], "command": "save", "context": [{"key": "auto_complete_visible"}] },
            { "keys": ["ctrl+alt+v"], "command": "save", "context": [{"key": "setting.is_widget"}] },
        ]"#,
        );
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings[0].contains("auto_complete_visible") && warnings[0].starts_with("line 3"), "{warnings:?}");
        let b = k.bindings().iter().find(|b| b.action == Some(Target::Cmd(Cmd::ToggleTerminal)) && !b.conds.is_empty()).unwrap();
        assert_eq!(b.conds, [Cond { key: CtxKey::TerminalFocus, want: true }]);
        // A context binding doesn't replace the context-free default.
        assert_eq!(k.strokes(Target::Cmd(Cmd::ToggleTerminal)), [(CTRL, Key::Backtick)]);
        let editor = KeyContext { editor: true, ..Default::default() };
        let term = KeyContext { terminal: true, panel_focus: true, ..Default::default() };
        assert!(!k.active(b, &editor));
        assert!(k.active(b, &term));
    }

    #[test]
    fn invalid_files_are_errors() {
        assert!(Keymap::with_user("{ \"keys\": [] }").unwrap_err().contains("list"));
        assert!(Keymap::with_user("[ { \"keys\": [\"ctrl+a\"], ").is_err());
        assert!(Keymap::with_user("  // nothing yet\n").is_ok());
        assert!(Keymap::with_user(&template()).unwrap().1.is_empty(), "the template is valid and empty");
    }

    #[test]
    fn defaults_text_is_valid_and_lists_every_command() {
        let text = defaults_text();
        let (entries, warnings) = parse_file(&text).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(entries.len(), Keymap::defaults().bindings().len());
        for &c in Cmd::ALL {
            assert!(text.contains(&format!("\"command\": \"{}\"", c.id())), "{}", c.id());
        }
        assert!(text.contains("\"command\": \"focus_group\", \"args\": {\"group\":1}"));
        assert!(text.contains("// { \"keys\": [], \"command\": \"toggle_minimap\" },"));
        // The listing re-creates the defaults exactly.
        let mut k = Keymap { bindings: Vec::new() };
        let mut w = Vec::new();
        k.apply(entries, &mut w);
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(k.bindings.len(), Keymap::defaults().bindings.len());
    }

    #[test]
    fn invalid_file_keeps_the_previous_keymap() {
        let dir = std::env::temp_dir().join(format!("strata-keys-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(FILE_NAME);
        // A key no default uses on any system, so loading it says nothing.
        let key = (CTRL_ALT.plus(Modifiers::SHIFT), Key::F24);
        std::fs::write(&path, r#"[ { "keys": ["ctrl+alt+shift+f24"], "command": "command_palette" } ]"#).unwrap();
        let mut file = KeymapFile::new(Some(&dir));
        assert_eq!(file.load(true), None);
        assert_eq!(current().strokes(Target::Cmd(Cmd::CommandPalette)), [key]);
        assert_eq!(file.poll(), None, "unchanged");

        std::fs::write(&path, "[ { \"keys\": [\"ctrl+alt+shift+f24\"],\r\n \"command\": \"save\" ").unwrap();
        let msg = file.poll().expect("noticed the change");
        assert!(msg.contains("line 2") && msg.contains("keeping the previous key bindings"), "{msg}");
        assert_eq!(current().strokes(Target::Cmd(Cmd::CommandPalette)), [key], "still the last good keymap");

        std::fs::remove_file(&path).unwrap();
        assert!(file.poll().unwrap().contains("defaults"));
        assert_eq!(current().strokes(Target::Cmd(Cmd::CommandPalette)), [(CMD_SHIFT, Key::P)]);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn dispatch_takes_single_keys_and_chords_in_order() {
        let ctx = egui::Context::default();
        let k = Keymap::defaults();
        let mut chord = Chord::default();
        let cmd = if MAC { Modifiers::MAC_CMD.plus(Modifiers::COMMAND) } else { Modifiers::CTRL.plus(Modifiers::COMMAND) };
        let key = |key, modifiers| Event::Key { key, physical_key: Some(key), pressed: true, repeat: false, modifiers };
        let editor = KeyContext { editor: true, ..Default::default() };
        let run = |events: Vec<Event>, kc: KeyContext, chord: &mut Chord| {
            let mut out = Vec::new();
            let mut frame = ctx.run_ui(egui::RawInput { events, ..Default::default() }, |ui| {
                out = k.dispatch(ui.ctx(), &kc, chord).into_iter().map(|f| f.action).collect::<Vec<_>>();
                ui.ctx().input(|i| assert!(i.events.iter().all(|e| !matches!(e, Event::Key { key: Key::S, .. }))));
            });
            frame.textures_delta.clear();
            out
        };
        assert_eq!(run(vec![key(Key::S, cmd)], editor, &mut chord), [Some(Target::Cmd(Cmd::Save))]);
        assert_eq!(run(vec![key(Key::K, cmd)], editor, &mut chord), []);
        assert!(chord.pending());
        assert_eq!(run(vec![key(Key::U, cmd)], editor, &mut chord), [Some(Target::Editor("upper"))]);
        assert!(!chord.pending());
        // Editor commands need the editor; window commands don't.
        let field = KeyContext::default();
        assert_eq!(run(vec![key(Key::D, cmd), key(Key::S, cmd)], field, &mut chord), [Some(Target::Cmd(Cmd::Save))]);
        // Sublime's Cmd+K Cmd+B shows the side bar, from the editor or not.
        assert_eq!(run(vec![key(Key::K, cmd), key(Key::B, cmd)], editor, &mut chord), [Some(Target::Cmd(Cmd::ToggleSidebar))]);
        assert_eq!(run(vec![key(Key::K, cmd), key(Key::B, cmd)], KeyContext::default(), &mut chord), [Some(Target::Cmd(Cmd::ToggleSidebar))]);
        // Escape cancels a chord.
        assert_eq!(run(vec![key(Key::K, cmd), key(Key::Escape, Modifiers::NONE), key(Key::U, cmd)], editor, &mut chord), [Some(Target::Editor("soft_undo"))]);
    }

    #[test]
    fn a_bound_keys_text_is_not_typed() {
        let (k, _) = user(r#"[ { "keys": ["alt+x"], "command": "save" } ]"#);
        let ctx = egui::Context::default();
        let alt_x = Event::Key { key: Key::X, physical_key: Some(Key::X), pressed: true, repeat: false, modifiers: Modifiers::ALT };
        let mut left = Vec::new();
        let mut fired = 0;
        let mut out = ctx.run_ui(egui::RawInput { events: vec![alt_x, Event::Text("≈".into()), Event::Text("y".into())], ..Default::default() }, |ui| {
            fired = k.dispatch(ui.ctx(), &KeyContext::default(), &mut Chord::default()).len();
            left = ui.ctx().input(|i| i.events.clone());
        });
        out.textures_delta.clear();
        assert_eq!(fired, 1);
        assert_eq!(left, [Event::Text("y".into())]);
    }

    #[test]
    fn menu_keys_show_chords_as_text() {
        let k = Keymap::defaults();
        assert_eq!(k.menu_keys(Target::Cmd(Cmd::CommandPalette)), (Some((CMD_SHIFT, Key::P)), None));
        assert_eq!(k.menu_keys(Target::Editor("upper")), (None, Some(format!("{} {}", label(CMD, Key::K), label(CMD, Key::U)))));
        assert_eq!(k.menu_keys(Target::Cmd(Cmd::ToggleMinimap)), (None, None));
    }
}
