//! The plugin API's methods (what a plugin may ask of Strata), the
//! permission each one needs, the error codes, and the text positions it
//! uses: 0-based lines and columns counted in Unicode characters (code
//! points), so a Python plugin can index its strings directly.

use super::Perm;
use ropey::Rope;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::ops::Range;

// Standard JSON-RPC errors.
pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;
// Strata's own.
pub const PERMISSION_DENIED: i64 = -32010;
pub const DOCUMENT_CHANGED: i64 = -32011;
pub const NO_DOCUMENT: i64 = -32012;
pub const OUTSIDE_WORKSPACE: i64 = -32013;
pub const TOO_LARGE: i64 = -32014;
pub const BUSY: i64 = -32015;
pub const READ_ONLY: i64 = -32016;
pub const IO_ERROR: i64 = -32017;
/// Strata's answer to a request of its own that the plugin didn't answer in time.
pub const TIMED_OUT: i64 = -32018;
/// The plugin stopped before answering.
pub const STOPPED: i64 = -32019;

/// Largest text a plugin may send or receive in one message (a document,
/// a file, a panel).
pub const MAX_TEXT: usize = 16 << 20;
const MAX_EDITS: usize = 100_000;

#[derive(Clone, Debug, PartialEq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

impl RpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }

    pub fn params(message: impl Into<String>) -> Self {
        Self::new(INVALID_PARAMS, message)
    }

    pub fn to_json(&self) -> Value {
        json!({"code": self.code, "message": self.message})
    }
}

pub type Reply = Result<Value, RpcError>;

/// Who carries a method out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Where {
    /// The plugin's own reader thread (files, the log): no window needed.
    Host,
    /// A window: documents and things on screen.
    Window,
}

pub struct Method {
    pub name: &'static str,
    /// Any one of these is enough; empty: no permission needed.
    pub needs: &'static [Perm],
    pub at: Where,
}

const fn m(name: &'static str, needs: &'static [Perm], at: Where) -> Method {
    Method { name, needs, at }
}

/// Every method a plugin may call.
pub const METHODS: &[Method] = &[
    m("document/read", &[Perm::ReadDocument], Where::Window),
    m("document/applyEdits", &[Perm::EditDocument], Where::Window),
    m("workspace/folders", &[Perm::ReadWorkspace, Perm::WriteWorkspace], Where::Host),
    m("workspace/readFile", &[Perm::ReadWorkspace], Where::Host),
    m("workspace/writeFile", &[Perm::WriteWorkspace], Where::Host),
    m("ui/showPanel", &[Perm::Panel], Where::Window),
    m("ui/notify", &[Perm::Notify], Where::Window),
    m("ui/statusItem", &[Perm::Notify], Where::Window),
    m("ui/pick", &[Perm::Panel], Where::Window),
    m("log", &[], Where::Host),
];

/// The method, if `granted` allows it. Every request and notification a
/// plugin sends goes through here first.
pub fn check(method: &str, granted: &BTreeSet<Perm>) -> Result<&'static Method, RpcError> {
    let Some(m) = METHODS.iter().find(|m| m.name == method) else {
        let shown: String = method.chars().take(80).collect();
        return Err(RpcError::new(METHOD_NOT_FOUND, format!("Strata has no method \"{shown}\"")));
    };
    if !m.needs.is_empty() && !m.needs.iter().any(|p| granted.contains(p)) {
        let names: Vec<&str> = m.needs.iter().map(|p| p.id()).collect();
        return Err(RpcError::new(PERMISSION_DENIED, format!("{method} needs the {} permission, which this plugin wasn't granted", names.join(" or "))));
    }
    Ok(m)
}

/// A string parameter, required.
pub fn str_param<'a>(params: &'a Value, key: &str) -> Result<&'a str, RpcError> {
    params.get(key).and_then(Value::as_str).ok_or_else(|| RpcError::params(format!("\"{key}\" must be a string")))
}

/// `{"line", "character"}` of char `idx` (characters counted as code points).
pub fn position(rope: &Rope, idx: usize) -> Value {
    let idx = idx.min(rope.len_chars());
    let line = rope.char_to_line(idx);
    json!({"line": line, "character": idx - rope.line_to_char(line)})
}

/// The char index of `{"line", "character"}`: a line past the last is an
/// error; a column past the line's end means its end (before the break).
pub fn char_index(rope: &Rope, v: &Value) -> Result<usize, RpcError> {
    let num = |k: &str| v.get(k).and_then(Value::as_u64).ok_or_else(|| RpcError::params(format!("a position needs a whole-number \"{k}\"")));
    let (line, col) = (num("line")?, num("character")?);
    let lines = rope.len_lines() as u64;
    if line >= lines {
        return Err(RpcError::params(format!("line {line} is past the end of the document ({lines} lines)")));
    }
    let line = line as usize;
    let start = rope.line_to_char(line);
    let slice = rope.line(line);
    let mut len = slice.len_chars();
    while len > 0 && matches!(slice.char(len - 1), '\n' | '\r') {
        len -= 1;
    }
    Ok(start + (col.min(len as u64) as usize))
}

/// `[{"range": {"start", "end"}, "newText"}]` as char edits, sorted, with
/// the new text's line breaks made `eol`. Overlapping edits are an error.
pub fn char_edits(rope: &Rope, edits: &Value, eol: &str) -> Result<Vec<(Range<usize>, String)>, RpcError> {
    let list = edits.as_array().ok_or_else(|| RpcError::params("\"edits\" must be an array"))?;
    if list.len() > MAX_EDITS {
        return Err(RpcError::new(TOO_LARGE, format!("more than {MAX_EDITS} edits")));
    }
    let mut out = Vec::with_capacity(list.len());
    let mut bytes = 0usize;
    for e in list {
        let range = e.get("range").ok_or_else(|| RpcError::params("an edit needs a \"range\""))?;
        let a = char_index(rope, range.get("start").ok_or_else(|| RpcError::params("a range needs a \"start\""))?)?;
        let b = char_index(rope, range.get("end").ok_or_else(|| RpcError::params("a range needs an \"end\""))?)?;
        if b < a {
            return Err(RpcError::params("a range ends before it starts"));
        }
        let text = str_param(e, "newText")?;
        bytes += text.len();
        if bytes > MAX_TEXT {
            return Err(RpcError::new(TOO_LARGE, "the edits' new text is larger than 16 MB"));
        }
        out.push((a..b, crate::lsp::edit::normalize_breaks(text, eol)));
    }
    // Stable: inserts at one spot keep the plugin's order.
    out.sort_by_key(|(r, _)| r.start);
    if out.windows(2).any(|w| w[0].0.end > w[1].0.start) {
        return Err(RpcError::params("the edits overlap"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> BTreeSet<Perm> {
        Perm::ALL.iter().copied().collect()
    }

    #[test]
    fn every_method_needs_its_permission() {
        // Each permission-gated method is refused without it and allowed with it.
        for m in METHODS {
            if m.needs.is_empty() {
                assert!(check(m.name, &BTreeSet::new()).is_ok(), "{}", m.name);
                continue;
            }
            let e = check(m.name, &BTreeSet::new()).err().unwrap_or_else(|| panic!("{} allowed with nothing granted", m.name));
            assert_eq!(e.code, PERMISSION_DENIED, "{}", m.name);
            assert!(e.message.contains(m.needs[0].id()), "{}", e.message);
            // Everything but what it needs: still refused.
            let others: BTreeSet<Perm> = all().into_iter().filter(|p| !m.needs.contains(p)).collect();
            assert_eq!(check(m.name, &others).err().map(|e| e.code), Some(PERMISSION_DENIED), "{}", m.name);
            for p in m.needs {
                assert!(check(m.name, &BTreeSet::from([*p])).is_ok(), "{} with {}", m.name, p.id());
            }
        }
        let expect = [
            ("document/read", Perm::ReadDocument),
            ("document/applyEdits", Perm::EditDocument),
            ("workspace/readFile", Perm::ReadWorkspace),
            ("workspace/writeFile", Perm::WriteWorkspace),
            ("ui/showPanel", Perm::Panel),
            ("ui/pick", Perm::Panel),
            ("ui/notify", Perm::Notify),
            ("ui/statusItem", Perm::Notify),
        ];
        for (method, perm) in expect {
            assert_eq!(METHODS.iter().find(|m| m.name == method).unwrap().needs, [perm], "{method}");
        }
        // Reading a file doesn't let a plugin write one, and the reverse.
        assert!(check("workspace/writeFile", &BTreeSet::from([Perm::ReadWorkspace])).is_err());
        assert!(check("workspace/readFile", &BTreeSet::from([Perm::WriteWorkspace])).is_err());
        // run_process and network grant no method.
        assert!(METHODS.iter().all(|m| !m.needs.contains(&Perm::RunProcess) && !m.needs.contains(&Perm::Network)));
    }

    #[test]
    fn unknown_methods() {
        let e = check("workspace/deleteEverything", &all()).err().unwrap();
        assert_eq!(e.code, METHOD_NOT_FOUND);
        assert!(check("Document/Read", &all()).is_err(), "names are exact");
    }

    #[test]
    fn positions_count_code_points() {
        let rope = Rope::from_str("a👋b\r\nline two\n");
        assert_eq!(position(&rope, 2), json!({"line": 0, "character": 2}));
        assert_eq!(position(&rope, 5), json!({"line": 1, "character": 0}));
        assert_eq!(char_index(&rope, &json!({"line": 0, "character": 2})).unwrap(), 2);
        // Past the end of a line: its end, before \r\n.
        assert_eq!(char_index(&rope, &json!({"line": 0, "character": 99})).unwrap(), 3);
        // The empty last line exists; one past it doesn't.
        assert_eq!(char_index(&rope, &json!({"line": 2, "character": 0})).unwrap(), rope.len_chars());
        assert_eq!(char_index(&rope, &json!({"line": 3, "character": 0})).unwrap_err().code, INVALID_PARAMS);
        assert!(char_index(&rope, &json!({"line": -1, "character": 0})).is_err());
    }

    #[test]
    fn edits_are_checked_sorted_and_keep_line_endings() {
        let rope = Rope::from_str("one\r\ntwo\r\n");
        let e = |l0: u32, c0: u32, l1: u32, c1: u32, t: &str| json!({"range": {"start": {"line": l0, "character": c0}, "end": {"line": l1, "character": c1}}, "newText": t});
        let edits = json!([e(1, 0, 1, 3, "TWO"), e(0, 0, 0, 0, "zero\n")]);
        let out = char_edits(&rope, &edits, "\r\n").unwrap();
        assert_eq!(out, vec![(0..0, "zero\r\n".to_owned()), (5..8, "TWO".to_owned())]);
        assert_eq!(char_edits(&rope, &json!([e(0, 0, 0, 3, "a"), e(0, 2, 0, 3, "b")]), "\n").unwrap_err().message, "the edits overlap");
        assert!(char_edits(&rope, &json!([e(0, 3, 0, 1, "a")]), "\n").unwrap_err().message.contains("ends before"));
        assert!(char_edits(&rope, &json!({"not": "a list"}), "\n").is_err());
        assert!(char_edits(&rope, &json!([{"range": {"start": {"line": 0, "character": 0}}, "newText": ""}]), "\n").is_err());
        assert!(char_edits(&rope, &json!([e(0, 0, 0, 0, "")]), "\n").unwrap()[0].1.is_empty());
    }
}
