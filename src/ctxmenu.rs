//! Right-click menus: one popup helper that draws `menus::Item`s exactly like
//! the app's menus, and a Cut / Copy / Paste / Select All menu for text fields.

use crate::keys::CMD;
use crate::menus::{Act, Item};
use egui::text::{CCursor, CCursorRange};
use egui::{Id, Key, Response, Ui};

/// Show `items` as a context menu for `resp`. It opens on a secondary click
/// of `resp`, or when `open` is set (for areas whose widgets eat the click).
/// The items are built only while the menu is open. Returns the chosen action.
pub fn popup(resp: &Response, open: bool, items: impl FnOnce() -> Vec<Item>) -> Option<Act> {
    let mut chosen = None;
    let mut popup = egui::Popup::context_menu(resp);
    if open {
        popup = popup.open_memory(egui::SetOpenCommand::Bool(true));
    }
    popup.show(|ui| {
        ui.set_min_width(200.0);
        crate::menus::items_ui(ui, &items(), &mut chosen);
    });
    chosen
}

/// A secondary click inside `rect` this frame that no window or popup above
/// `layer` caught: for panels whose content widgets take the click themselves.
pub fn secondary_click_in(ctx: &egui::Context, rect: egui::Rect, layer: egui::LayerId) -> bool {
    let Some(pos) = ctx.input(|i| i.pointer.secondary_clicked().then(|| i.pointer.interact_pos()).flatten()) else { return false };
    rect.contains(pos) && ctx.layer_id_at(pos).is_none_or(|l| l == layer)
}

/// The standard clipboard rows.
pub fn clipboard_items(can_cut: bool, can_copy: bool, can_paste: bool) -> Vec<Item> {
    vec![
        Item::new("Cut", Act::Cut).key(CMD, Key::X).enabled(can_cut),
        Item::new("Copy", Act::Copy).key(CMD, Key::C).enabled(can_copy),
        Item::new("Paste", Act::Paste).key(CMD, Key::V).enabled(can_paste),
        Item::Separator,
        Item::new("Select All", Act::SelectAll).key(CMD, Key::A),
    ]
}

/// Text from the system clipboard, the same way the Edit menu's Paste reads it.
pub fn clipboard_text() -> Option<String> {
    arboard::Clipboard::new().and_then(|mut c| c.get_text()).ok()
}

/// A single-line text field with a Cut / Copy / Paste / Select All menu.
/// `add` draws the field (it must use `id`). Right-clicking keeps the field's
/// selection, so Copy and Cut act on what was selected.
pub fn text_field(ui: &mut Ui, id: Id, text: &mut String, add: impl FnOnce(&mut Ui, &mut String) -> Response) -> Response {
    let ctx = ui.ctx().clone();
    let right_press = ui.input(|i| i.pointer.button_pressed(egui::PointerButton::Secondary));
    let before = right_press.then(|| egui::TextEdit::load_state(&ctx, id)).flatten();
    let resp = add(ui, text);
    // egui moves the caret on any button press; put a selection back.
    if let Some(state) = before
        && resp.hovered()
        && state.cursor.char_range().is_some_and(|r| !r.is_empty())
    {
        egui::TextEdit::store_state(&ctx, id, state);
    }
    let selected = !selection(&ctx, id, text).is_empty();
    if let Some(act) = popup(&resp, false, || clipboard_items(selected, selected, true)) {
        let pasted = if act == Act::Paste { clipboard_text() } else { None };
        edit_field(&ctx, id, text, &act, pasted.as_deref());
    }
    resp
}

/// The field's selection in chars (empty at the caret; the end with no state).
fn selection(ctx: &egui::Context, id: Id, text: &str) -> std::ops::Range<usize> {
    let n = text.chars().count();
    let range = egui::TextEdit::load_state(ctx, id).and_then(|s| s.cursor.char_range());
    match range {
        Some(r) => {
            let [a, b] = r.sorted_cursors();
            a.index.0.min(n)..b.index.0.min(n)
        }
        None => n..n,
    }
}

/// Apply a clipboard menu action to the text field `id`, then focus it.
/// `pasted` is the clipboard text for `Act::Paste` (newlines become spaces).
pub fn edit_field(ctx: &egui::Context, id: Id, text: &mut String, act: &Act, pasted: Option<&str>) {
    let r = selection(ctx, id, text);
    let byte = |text: &str, c: usize| text.char_indices().nth(c).map_or(text.len(), |(b, _)| b);
    let (a, b) = (byte(text, r.start), byte(text, r.end));
    let caret = match act {
        Act::Copy | Act::Cut if r.is_empty() => return,
        Act::Copy => {
            ctx.copy_text(text[a..b].to_owned());
            r
        }
        Act::Cut => {
            ctx.copy_text(text[a..b].to_owned());
            text.replace_range(a..b, "");
            r.start..r.start
        }
        Act::Paste => {
            let Some(p) = pasted else { return };
            let p = p.replace("\r\n", " ").replace(['\n', '\r'], " ");
            text.replace_range(a..b, &p);
            let end = r.start + p.chars().count();
            end..end
        }
        Act::SelectAll => 0..text.chars().count(),
        _ => return,
    };
    let mut state = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
    state.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(caret.start), CCursor::new(caret.end))));
    egui::TextEdit::store_state(ctx, id, state);
    ctx.memory_mut(|m| m.request_focus(id));
}

/// Show `path` in Finder (selected), Explorer (selected), or its folder elsewhere.
pub fn reveal_in_file_manager(path: &std::path::Path) -> std::io::Result<()> {
    let mut cmd = if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        c.arg("-R").arg(path);
        c
    } else if cfg!(windows) {
        let mut c = std::process::Command::new("explorer");
        c.arg(format!("/select,{}", path.display()));
        c
    } else {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(path.parent().unwrap_or(path));
        c
    };
    cmd.spawn().map(|_| ())
}

/// "Reveal in Finder" / "Show in Explorer" / "Open Containing Folder".
pub fn reveal_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "Reveal in Finder"
    } else if cfg!(windows) {
        "Show in Explorer"
    } else {
        "Open Containing Folder"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_edits_follow_the_selection() {
        let ctx = egui::Context::default();
        let id = Id::new("t");
        let mut text = String::from("héllo world");
        let set = |a: usize, b: usize| {
            let mut s = egui::TextEdit::load_state(&ctx, id).unwrap_or_default();
            s.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(a), CCursor::new(b))));
            egui::TextEdit::store_state(&ctx, id, s);
        };
        set(0, 5);
        edit_field(&ctx, id, &mut text, &Act::Cut, None);
        assert_eq!(text, " world");
        assert_eq!(selection(&ctx, id, &text), 0..0);
        edit_field(&ctx, id, &mut text, &Act::Paste, Some("a\nb"));
        assert_eq!(text, "a b world");
        assert_eq!(selection(&ctx, id, &text), 3..3);
        edit_field(&ctx, id, &mut text, &Act::SelectAll, None);
        assert_eq!(selection(&ctx, id, &text), 0..9);
        edit_field(&ctx, id, &mut text, &Act::Copy, None);
        let copied = ctx.output(|o| o.commands.iter().any(|c| matches!(c, egui::OutputCommand::CopyText(t) if t == "a b world")));
        assert!(copied);
        assert!(ctx.memory(|m| m.has_focus(id)));
    }
}
