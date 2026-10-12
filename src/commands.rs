//! Window-level commands: one table drives the shortcuts and the command palette.

use crate::keys::*;
use egui::{Key, Modifiers};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cmd {
    NewTab,
    Open,
    OpenFolder,
    Save,
    SaveAs,
    SaveAll,
    CloseTab,
    CloseAll,
    ReopenClosed,
    NextTab,
    PrevTab,
    ToggleSidebar,
    Find,
    Replace,
    FindNext,
    FindPrev,
    FindInFiles,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    ToggleMinimap,
    Fullscreen,
    Settings,
    CommandPalette,
    GotoAnything,
    GotoLine,
    GotoSymbol,
    SelectTheme,
    SelectSyntax,
    LayoutSingle,
    LayoutColumns2,
    LayoutColumns3,
    LayoutColumns4,
    LayoutRows2,
    LayoutGrid,
    SplitRight,
    FocusGroup(u8),
    MoveToGroup(u8),
}

impl Cmd {
    pub const ALL: &[Cmd] = &[
        Cmd::NewTab,
        Cmd::Open,
        Cmd::OpenFolder,
        Cmd::Save,
        Cmd::SaveAs,
        Cmd::SaveAll,
        Cmd::CloseTab,
        Cmd::CloseAll,
        Cmd::ReopenClosed,
        Cmd::NextTab,
        Cmd::PrevTab,
        Cmd::ToggleSidebar,
        Cmd::Find,
        Cmd::Replace,
        Cmd::FindNext,
        Cmd::FindPrev,
        Cmd::FindInFiles,
        Cmd::ZoomIn,
        Cmd::ZoomOut,
        Cmd::ZoomReset,
        Cmd::ToggleMinimap,
        Cmd::Fullscreen,
        Cmd::Settings,
        Cmd::CommandPalette,
        Cmd::GotoAnything,
        Cmd::GotoLine,
        Cmd::GotoSymbol,
        Cmd::SelectTheme,
        Cmd::SelectSyntax,
        Cmd::LayoutSingle,
        Cmd::LayoutColumns2,
        Cmd::LayoutColumns3,
        Cmd::LayoutColumns4,
        Cmd::LayoutRows2,
        Cmd::LayoutGrid,
        Cmd::SplitRight,
        Cmd::FocusGroup(0),
        Cmd::FocusGroup(1),
        Cmd::FocusGroup(2),
        Cmd::FocusGroup(3),
        Cmd::MoveToGroup(0),
        Cmd::MoveToGroup(1),
        Cmd::MoveToGroup(2),
        Cmd::MoveToGroup(3),
    ];

    pub fn name(self) -> &'static str {
        match self {
            Cmd::NewTab => "File: New File",
            Cmd::Open => "File: Open…",
            Cmd::OpenFolder => "File: Open Folder…",
            Cmd::Save => "File: Save",
            Cmd::SaveAs => "File: Save As…",
            Cmd::SaveAll => "File: Save All",
            Cmd::CloseTab => "File: Close Tab",
            Cmd::CloseAll => "File: Close All Tabs",
            Cmd::ReopenClosed => "File: Reopen Closed Tab",
            Cmd::NextTab => "View: Next Tab",
            Cmd::PrevTab => "View: Previous Tab",
            Cmd::ToggleSidebar => "View: Toggle Sidebar",
            Cmd::Find => "Find: Find…",
            Cmd::Replace => "Find: Replace…",
            Cmd::FindNext => "Find: Find Next",
            Cmd::FindPrev => "Find: Find Previous",
            Cmd::FindInFiles => "Find: Find in Folder…",
            Cmd::ZoomIn => "View: Zoom In",
            Cmd::ZoomOut => "View: Zoom Out",
            Cmd::ZoomReset => "View: Reset Zoom",
            Cmd::ToggleMinimap => "View: Toggle Minimap",
            Cmd::Fullscreen => "View: Toggle Full Screen",
            Cmd::Settings => "Preferences: Settings",
            Cmd::CommandPalette => "Command Palette",
            Cmd::GotoAnything => "Goto: Anything (Files, :Line, @Symbol)",
            Cmd::GotoLine => "Goto: Line…",
            Cmd::GotoSymbol => "Goto: Symbol…",
            Cmd::SelectTheme => "Preferences: Color Scheme…",
            Cmd::SelectSyntax => "View: Syntax…",
            Cmd::LayoutSingle => "Layout: Single",
            Cmd::LayoutColumns2 => "Layout: Columns: 2",
            Cmd::LayoutColumns3 => "Layout: Columns: 3",
            Cmd::LayoutColumns4 => "Layout: Columns: 4",
            Cmd::LayoutRows2 => "Layout: Rows: 2",
            Cmd::LayoutGrid => "Layout: Grid: 4",
            Cmd::SplitRight => "Layout: Split Right (move file to a new pane)",
            Cmd::FocusGroup(0) => "Layout: Focus Pane 1",
            Cmd::FocusGroup(1) => "Layout: Focus Pane 2",
            Cmd::FocusGroup(2) => "Layout: Focus Pane 3",
            Cmd::FocusGroup(_) => "Layout: Focus Pane 4",
            Cmd::MoveToGroup(0) => "Layout: Move File to Pane 1",
            Cmd::MoveToGroup(1) => "Layout: Move File to Pane 2",
            Cmd::MoveToGroup(2) => "Layout: Move File to Pane 3",
            Cmd::MoveToGroup(_) => "Layout: Move File to Pane 4",
        }
    }

    /// Shortcuts, first one shown in the palette.
    pub fn keys(self) -> &'static [(Modifiers, Key)] {
        match self {
            Cmd::NewTab => &[(CMD, Key::N)],
            Cmd::Open => &[(CMD, Key::O)],
            Cmd::OpenFolder => &[(CMD_SHIFT, Key::O)],
            Cmd::Save => &[(CMD, Key::S)],
            Cmd::SaveAs => &[(CMD_SHIFT, Key::S)],
            Cmd::SaveAll => &[(CMD_ALT, Key::S)],
            Cmd::CloseTab => if MAC { &[(CMD, Key::W)] } else { &[(CMD, Key::W), (CTRL, Key::F4)] },
            Cmd::CloseAll => &[],
            Cmd::ReopenClosed => &[(CMD_SHIFT, Key::T)],
            Cmd::NextTab => if MAC {
                &[(CTRL, Key::Tab), (CMD_ALT, Key::ArrowRight), (CMD_SHIFT, Key::CloseBracket), (CMD_SHIFT, Key::CloseCurlyBracket), (CTRL, Key::PageDown)]
            } else {
                &[(CTRL, Key::Tab), (CTRL, Key::PageDown)]
            },
            Cmd::PrevTab => if MAC {
                &[(CTRL_SHIFT, Key::Tab), (CMD_ALT, Key::ArrowLeft), (CMD_SHIFT, Key::OpenBracket), (CMD_SHIFT, Key::OpenCurlyBracket), (CTRL, Key::PageUp)]
            } else {
                &[(CTRL_SHIFT, Key::Tab), (CTRL, Key::PageUp)]
            },
            Cmd::ToggleSidebar => &[(CMD, Key::Backslash)],
            Cmd::Find => &[(CMD, Key::F)],
            Cmd::Replace => if MAC { &[(CMD_ALT, Key::F)] } else { &[(CTRL, Key::H)] },
            // Off macOS, Ctrl+G is go to line, so find next is F3 only.
            Cmd::FindNext => if MAC { &[(CMD, Key::G), (NONE, Key::F3)] } else { &[(NONE, Key::F3)] },
            Cmd::FindPrev => if MAC { &[(CMD_SHIFT, Key::G), (SHIFT, Key::F3)] } else { &[(SHIFT, Key::F3)] },
            Cmd::FindInFiles => &[(CMD_SHIFT, Key::F)],
            Cmd::ZoomIn => &[(CMD, Key::Equals), (CMD, Key::Plus), (CMD_SHIFT, Key::Equals)],
            Cmd::ZoomOut => &[(CMD, Key::Minus)],
            Cmd::ZoomReset => &[(CMD, Key::Num0)],
            Cmd::ToggleMinimap => &[],
            Cmd::Fullscreen => if MAC { &[(CTRL_CMD, Key::F)] } else { &[(NONE, Key::F11)] },
            Cmd::Settings => &[(CMD, Key::Comma)],
            Cmd::CommandPalette => &[(CMD_SHIFT, Key::P)],
            Cmd::GotoAnything => &[(CMD, Key::P), (CMD, Key::T)],
            Cmd::GotoLine => &[(CTRL, Key::G)],
            Cmd::GotoSymbol => &[(CMD, Key::R)],
            Cmd::SelectTheme | Cmd::SelectSyntax | Cmd::SplitRight => &[],
            // Sublime's layout keys: Alt+Cmd+N on macOS, Alt+Shift+N elsewhere.
            Cmd::LayoutSingle => if MAC { &[(CMD_ALT, Key::Num1)] } else { &[(ALT_SHIFT, Key::Num1)] },
            Cmd::LayoutColumns2 => if MAC { &[(CMD_ALT, Key::Num2)] } else { &[(ALT_SHIFT, Key::Num2)] },
            Cmd::LayoutColumns3 => if MAC { &[(CMD_ALT, Key::Num3)] } else { &[(ALT_SHIFT, Key::Num3)] },
            Cmd::LayoutColumns4 => if MAC { &[(CMD_ALT, Key::Num4)] } else { &[(ALT_SHIFT, Key::Num4)] },
            Cmd::LayoutGrid => if MAC { &[(CMD_ALT, Key::Num5)] } else { &[(ALT_SHIFT, Key::Num5)] },
            Cmd::LayoutRows2 => if MAC { &[(CMD_ALT_SHIFT, Key::Num2)] } else { &[(ALT_SHIFT, Key::Num8)] },
            Cmd::FocusGroup(0) => &[(CTRL, Key::Num1)],
            Cmd::FocusGroup(1) => &[(CTRL, Key::Num2)],
            Cmd::FocusGroup(2) => &[(CTRL, Key::Num3)],
            Cmd::FocusGroup(_) => &[(CTRL, Key::Num4)],
            Cmd::MoveToGroup(0) => &[(CTRL_SHIFT, Key::Num1)],
            Cmd::MoveToGroup(1) => &[(CTRL_SHIFT, Key::Num2)],
            Cmd::MoveToGroup(2) => &[(CTRL_SHIFT, Key::Num3)],
            Cmd::MoveToGroup(_) => &[(CTRL_SHIFT, Key::Num4)],
        }
    }

    pub fn shortcut_label(self) -> Option<String> {
        self.keys().first().map(|&(m, k)| label(m, k))
    }
}

/// Shortcuts that never reach the editor (the editor handles its own keys).
pub fn pressed(ctx: &egui::Context) -> Vec<Cmd> {
    let mut out = Vec::new();
    for &cmd in Cmd::ALL {
        for &(mods, key) in cmd.keys() {
            if take(ctx, mods, key) {
                out.push(cmd);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_two_commands_share_a_shortcut() {
        let mut seen = Vec::new();
        for &cmd in Cmd::ALL {
            for &(m, k) in cmd.keys() {
                let l = label(m, k);
                assert!(!seen.contains(&l), "{l} bound twice");
                seen.push(l);
            }
        }
    }
}
