//! Text edits from a server (rename, formatting, completion): reading them
//! out of the protocol's shapes and turning them into char ranges.

use super::position::{self, Encoding, Pos};
use ropey::Rope;
use serde_json::Value;
use std::ops::Range;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq)]
pub struct TextEdit {
    pub range: (Pos, Pos),
    pub text: String,
}

impl TextEdit {
    pub fn from_json(v: &Value) -> Option<Self> {
        // An InsertReplaceEdit (completion) has `insert` and `replace` ranges; use `replace`.
        let range = v.get("range").or_else(|| v.get("replace"))?;
        Some(Self { range: position::range_from_json(range)?, text: v.get("newText")?.as_str()?.to_owned() })
    }
}

/// The edits for one file.
#[derive(Clone, Debug, PartialEq)]
pub struct FileEdit {
    pub path: PathBuf,
    /// The document version the edits were made for, when the server says.
    pub version: Option<i32>,
    pub edits: Vec<TextEdit>,
}

/// A list of TextEdits (formatting's answer); bad entries are an error.
pub fn text_edits(v: &Value) -> Result<Vec<TextEdit>, String> {
    match v {
        Value::Null => Ok(Vec::new()),
        Value::Array(a) => a.iter().map(|e| TextEdit::from_json(e).ok_or_else(|| "the server sent a malformed edit".to_owned())).collect(),
        _ => Err("the server sent a malformed edit".into()),
    }
}

/// The text changes of a WorkspaceEdit, by file (files in the order the
/// server listed them). Creating, renaming or deleting files isn't
/// supported (Strata doesn't announce it): such an edit is refused whole,
/// so nothing is half done.
pub fn workspace_edit(v: &Value) -> Result<Vec<FileEdit>, String> {
    let malformed = || "the server sent a malformed edit".to_owned();
    let mut out: Vec<FileEdit> = Vec::new();
    let mut add = |path: PathBuf, version: Option<i32>, edits: Vec<TextEdit>| match out.iter_mut().find(|f| f.path == path) {
        Some(f) => f.edits.extend(edits),
        None => out.push(FileEdit { path, version, edits }),
    };
    if let Some(changes) = v.get("documentChanges").and_then(Value::as_array) {
        for c in changes {
            if c.get("kind").is_some() {
                return Err("the change also creates, renames or deletes files, which Strata can't do yet".into());
            }
            let doc = c.get("textDocument").ok_or_else(malformed)?;
            let uri = doc.get("uri").and_then(Value::as_str).ok_or_else(malformed)?;
            let path = super::uri::to_path(uri).ok_or_else(|| format!("the change is for {uri}, which isn't a file"))?;
            let version = doc.get("version").and_then(Value::as_i64).map(|v| v as i32);
            let edits = text_edits(c.get("edits").unwrap_or(&Value::Null))?;
            add(super::uri::normalize(&path), version, edits);
        }
    } else if let Some(changes) = v.get("changes").and_then(Value::as_object) {
        for (uri, edits) in changes {
            let path = super::uri::to_path(uri).ok_or_else(|| format!("the change is for {uri}, which isn't a file"))?;
            add(super::uri::normalize(&path), None, text_edits(edits)?);
        }
    }
    Ok(out)
}

/// Edits as char ranges in `rope`, sorted by position (edits at the same
/// spot keep the server's order, as the protocol asks). Line breaks in the
/// new text become `line_ending`, so a CRLF file stays CRLF. Overlapping
/// edits are an error: the protocol forbids them.
pub fn to_char_edits(rope: &Rope, edits: &[TextEdit], enc: Encoding, line_ending: &str) -> Result<Vec<(Range<usize>, String)>, String> {
    let mut out: Vec<(Range<usize>, String)> = edits
        .iter()
        .map(|e| {
            let a = position::from_lsp(rope, e.range.0, enc);
            let b = position::from_lsp(rope, e.range.1, enc).max(a);
            (a..b, normalize_breaks(&e.text, line_ending))
        })
        .collect();
    out.sort_by_key(|(r, _)| r.start);
    if out.windows(2).any(|w| w[0].0.end > w[1].0.start) {
        return Err("the server sent overlapping edits".into());
    }
    Ok(out)
}

/// `\r\n`, `\r` and `\n` all become `ending`.
pub fn normalize_breaks(text: &str, ending: &str) -> String {
    if !text.contains(['\r', '\n']) {
        return text.to_owned();
    }
    text.replace("\r\n", "\n").replace('\r', "\n").replace('\n', ending)
}

/// Apply sorted, non-overlapping char edits to a plain string (for files
/// that aren't open; open files go through the editor, for undo).
#[cfg(test)]
pub fn apply_to_string(text: &str, edits: &[(Range<usize>, String)]) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut at = 0;
    for (r, t) in edits {
        out.extend(&chars[at..r.start]);
        out.push_str(t);
        at = r.end;
    }
    out.extend(&chars[at..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn te(l0: u32, c0: u32, l1: u32, c1: u32, text: &str) -> TextEdit {
        TextEdit { range: (Pos::new(l0, c0), Pos::new(l1, c1)), text: text.into() }
    }

    #[test]
    fn workspace_edits_by_file_in_both_shapes() {
        let (a, b) = if cfg!(windows) { ("file:///C:/p/a.rs", "file:///C:/p/b.rs") } else { ("file:///p/a.rs", "file:///p/b.rs") };
        let pa = super::super::uri::to_path(a).unwrap();
        let edit = |l: u32| json!({"range": {"start": {"line": l, "character": 0}, "end": {"line": l, "character": 3}}, "newText": "new"});
        let changes = json!({"changes": {a: [edit(0), edit(2)], b: [edit(1)]}});
        let files = workspace_edit(&changes).unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files.iter().find(|f| f.path == pa).unwrap().edits.len(), 2);

        let doc_changes = json!({"documentChanges": [
            {"textDocument": {"uri": a, "version": 4}, "edits": [edit(0)]},
            {"textDocument": {"uri": b, "version": null}, "edits": [edit(1)]},
            {"textDocument": {"uri": a, "version": 4}, "edits": [edit(5)]},
        ]});
        let files = workspace_edit(&doc_changes).unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!((files[0].version, files[0].edits.len()), (Some(4), 2));
        assert_eq!(files[1].version, None);

        let with_rename = json!({"documentChanges": [{"kind": "rename", "oldUri": a, "newUri": b}]});
        assert!(workspace_edit(&with_rename).is_err(), "file operations are refused whole");
        assert!(workspace_edit(&json!({"changes": {a: [{"range": 1}]}})).is_err());
        assert_eq!(workspace_edit(&json!({})).unwrap(), vec![]);
    }

    #[test]
    fn edits_are_sorted_with_ties_in_server_order() {
        let rope = Rope::from_str("abc def\nghi\n");
        // Given out of order; two inserts at the same spot keep their order.
        let edits = vec![te(1, 0, 1, 3, "GHI"), te(0, 4, 0, 4, "1"), te(0, 4, 0, 4, "2"), te(0, 0, 0, 3, "X")];
        let ce = to_char_edits(&rope, &edits, Encoding::Utf16, "\n").unwrap();
        assert_eq!(apply_to_string("abc def\nghi\n", &ce), "X 12def\nGHI\n");
        let overlapping = vec![te(0, 0, 0, 5, "a"), te(0, 3, 0, 6, "b")];
        assert!(to_char_edits(&rope, &overlapping, Encoding::Utf16, "\n").is_err());
    }

    #[test]
    fn crlf_files_keep_their_line_breaks_and_positions() {
        let text = "fn a() {\r\n    old();\r\n}\r\n";
        let rope = Rope::from_str(text);
        // Replace `old` on line 1 and insert two lines (written with \n) after line 0.
        let edits = vec![te(1, 4, 1, 7, "new"), te(0, 8, 0, 8, "\n    first();\n    second();")];
        let ce = to_char_edits(&rope, &edits, Encoding::Utf16, "\r\n").unwrap();
        assert_eq!(apply_to_string(text, &ce), "fn a() {\r\n    first();\r\n    second();\r\n    new();\r\n}\r\n");
        assert_eq!(normalize_breaks("a\r\nb\rc\nd", "\n"), "a\nb\nc\nd");
    }

    #[test]
    fn emoji_columns_in_edits() {
        // The server counts 👋 as two UTF-16 units: `x` starts at column 3.
        let text = "a👋x = 1;\n";
        let rope = Rope::from_str(text);
        let ce = to_char_edits(&rope, &[te(0, 3, 0, 4, "renamed")], Encoding::Utf16, "\n").unwrap();
        assert_eq!(apply_to_string(text, &ce), "a👋renamed = 1;\n");
        // The same edit from a server using UTF-8 offsets (👋 is four bytes).
        let ce = to_char_edits(&rope, &[te(0, 5, 0, 6, "renamed")], Encoding::Utf8, "\n").unwrap();
        assert_eq!(apply_to_string(text, &ce), "a👋renamed = 1;\n");
    }
}
