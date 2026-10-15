//! Shortcut modifiers and labels. egui's `consume_shortcut` ignores extra Shift/Alt
//! (so Cmd+G would also eat Ctrl+Cmd+G); the keymap matches them exactly.

use egui::{Key, Modifiers};

pub const MAC: bool = cfg!(target_os = "macos");

/// Cmd on macOS, Ctrl elsewhere.
pub const CMD: Modifiers = Modifiers::COMMAND;
pub const CMD_SHIFT: Modifiers = Modifiers::COMMAND.plus(Modifiers::SHIFT);
pub const CMD_ALT: Modifiers = Modifiers::COMMAND.plus(Modifiers::ALT);
pub const CTRL: Modifiers = Modifiers::CTRL;
pub const CTRL_SHIFT: Modifiers = Modifiers::CTRL.plus(Modifiers::SHIFT);
pub const CTRL_ALT: Modifiers = Modifiers::CTRL.plus(Modifiers::ALT);
pub const ALT_SHIFT: Modifiers = Modifiers::ALT.plus(Modifiers::SHIFT);
pub const CMD_ALT_SHIFT: Modifiers = Modifiers::COMMAND.plus(Modifiers::ALT).plus(Modifiers::SHIFT);
pub const NONE: Modifiers = Modifiers::NONE;
/// macOS only: Ctrl+Cmd.
pub const CTRL_CMD: Modifiers = Modifiers::MAC_CMD.plus(Modifiers::CTRL);
pub const CTRL_CMD_SHIFT: Modifiers = Modifiers::MAC_CMD.plus(Modifiers::CTRL).plus(Modifiers::SHIFT);
pub const SHIFT: Modifiers = Modifiers::SHIFT;
pub const ALT: Modifiers = Modifiers::ALT;

/// (alt, shift, cmd, ctrl) with Cmd and Ctrl folded together off macOS.
fn norm(m: Modifiers, pattern: bool) -> (bool, bool, bool, bool) {
    if MAC {
        let cmd = m.mac_cmd || (pattern && m.command);
        (m.alt, m.shift, cmd, m.ctrl)
    } else {
        let c = m.ctrl || m.command;
        (m.alt, m.shift, c, c)
    }
}

/// Two shortcut patterns that mean the same keys on this platform.
pub fn same_mods(a: Modifiers, b: Modifiers) -> bool {
    norm(a, true) == norm(b, true)
}

/// (alt, shift, cmd, ctrl) of a pattern, folded like `exact` compares them.
pub fn parts(m: Modifiers) -> (bool, bool, bool, bool) {
    norm(m, true)
}

pub fn exact(pressed: Modifiers, pattern: Modifiers) -> bool {
    norm(pressed, false) == norm(pattern, true)
}

/// A space after each modifier glyph, so "⇧⌘P" reads as three symbols.
pub fn spaced(label: &str) -> String {
    let mut out = String::with_capacity(label.len() * 2);
    for c in label.chars() {
        out.push(c);
        if matches!(c, '⌃' | '⌥' | '⇧' | '⌘') {
            out.push(' ');
        }
    }
    out
}

/// "⇧⌘P" on macOS, "Ctrl+Shift+P" elsewhere.
pub fn label(mods: Modifiers, key: Key) -> String {
    let (alt, shift, cmd, ctrl) = norm(mods, true);
    let name = match key {
        Key::OpenBracket => "[",
        Key::CloseBracket => "]",
        Key::Slash => "/",
        Key::Backslash => "\\",
        Key::Equals => "=",
        Key::Minus => "-",
        Key::Enter => if MAC { "↩" } else { "Enter" },
        Key::Tab => if MAC { "⇥" } else { "Tab" },
        Key::ArrowUp => "↑",
        Key::ArrowDown => "↓",
        Key::ArrowLeft => "←",
        Key::ArrowRight => "→",
        Key::Backtick => "`",
        Key::Comma => ",",
        Key::Period => ".",
        Key::Semicolon => ";",
        k => k.name(),
    };
    if MAC {
        let mut s = String::new();
        if ctrl {
            s.push('⌃');
        }
        if alt {
            s.push('⌥');
        }
        if shift {
            s.push('⇧');
        }
        if cmd {
            s.push('⌘');
        }
        spaced(&(s + name))
    } else {
        let mut parts = Vec::new();
        if ctrl {
            parts.push("Ctrl");
        }
        if alt {
            parts.push("Alt");
        }
        if shift {
            parts.push("Shift");
        }
        parts.push(name);
        parts.join("+")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_rejects_extra_modifiers() {
        let pressed_cmd = if MAC { Modifiers::MAC_CMD.plus(Modifiers::COMMAND) } else { Modifiers::CTRL.plus(Modifiers::COMMAND) };
        assert!(exact(pressed_cmd, CMD));
        assert!(!exact(pressed_cmd.plus(Modifiers::SHIFT), CMD));
        assert!(exact(pressed_cmd.plus(Modifiers::SHIFT), CMD_SHIFT));
        if MAC {
            assert!(!exact(pressed_cmd.plus(Modifiers::CTRL), CMD), "Ctrl+Cmd+G is not Cmd+G");
        }
    }
}
