//! A small language server for tests, so they don't need real servers.
//! The test binary runs itself as the server: `config` starts it with
//! `STRATA_FAKE_LSP` set, and the `fake_language_server` test then speaks
//! the protocol on stdin/stdout instead of returning at once.
//!
//! It keeps the text of open documents (applying incremental changes with
//! its own position arithmetic, independent of `position`), reports every
//! `bad` as an error and every `meh` as a warning, offers three completions,
//! answers hover, definition, references, rename (open files plus the files
//! of the same type in the project folder) and formatting (trailing spaces).
//! A document containing `CRASH_NOW` makes it exit, once (it leaves a
//! marker file so the restarted server stays up).
//!
//! `initializationOptions` set it up: `{"utf8": true}` prefers UTF-8
//! columns, `"marker"` is the crash marker's path, `"fail_to_start": true`
//! exits instead of answering `initialize`.

use super::config::ServerConfig;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};
use std::path::PathBuf;

/// A server entry that starts the fake server for files matching `patterns`.
pub fn config(name: &str, patterns: &[&str], options: Value) -> ServerConfig {
    let exe = std::env::current_exe().expect("the test binary's path");
    ServerConfig {
        name: name.into(),
        command: exe.to_string_lossy().into_owned(),
        args: ["--exact", "lsp::fake::fake_language_server", "--nocapture", "--test-threads=1", "-q"].iter().map(|s| s.to_string()).collect(),
        file_patterns: patterns.iter().map(|s| s.to_string()).collect(),
        root_markers: Vec::new(),
        initialization_options: Some(options),
        settings: None,
        language_id: None,
        env: BTreeMap::from([("STRATA_FAKE_LSP".to_owned(), "1".to_owned())]),
        builtin: false,
    }
}

#[test]
fn fake_language_server() {
    // In a normal test run this does nothing.
    if std::env::var_os("STRATA_FAKE_LSP").is_none() {
        return;
    }
    Fake::default().serve();
    std::process::exit(0);
}

/// A word and its range (start and end positions, as JSON).
type Word = (String, (Value, Value));

#[derive(Default)]
struct Fake {
    docs: HashMap<String, String>,
    utf8: bool,
    marker: Option<PathBuf>,
    root: Option<PathBuf>,
}

fn send(msg: &Value) {
    let body = msg.to_string();
    let mut out = std::io::stdout().lock();
    let _ = write!(out, "Content-Length: {}\r\n\r\n{body}", body.len());
    let _ = out.flush();
}

fn word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl Fake {
    fn serve(&mut self) {
        let mut stdin = std::io::stdin().lock();
        let mut decoder = super::jsonrpc::Decoder::default();
        let mut buf = [0u8; 65536];
        loop {
            let n = match stdin.read(&mut buf) {
                Ok(0) | Err(_) => return,
                Ok(n) => n,
            };
            decoder.push(&buf[..n]);
            while let Some(msg) = decoder.next() {
                let Ok(msg) = msg else { continue };
                if !self.handle(&msg) {
                    return;
                }
            }
        }
    }

    /// False to stop.
    fn handle(&mut self, m: &Value) -> bool {
        let method = m["method"].as_str().unwrap_or_default();
        let p = &m["params"];
        let reply = |result: Value| send(&json!({"jsonrpc": "2.0", "id": m["id"], "result": result}));
        match method {
            "initialize" => {
                let o = &p["initializationOptions"];
                if o["fail_to_start"].as_bool() == Some(true) {
                    // Like a rustup stub whose component isn't installed.
                    eprintln!("error: this server isn't really here");
                    std::process::exit(1);
                }
                let offered = p["capabilities"]["general"]["positionEncodings"].as_array().is_some_and(|a| a.iter().any(|e| e == "utf-8"));
                self.utf8 = o["utf8"].as_bool().unwrap_or(false) && offered;
                self.marker = o["marker"].as_str().map(PathBuf::from);
                self.root = p["rootUri"].as_str().and_then(super::uri::to_path);
                reply(json!({"capabilities": {
                    "positionEncoding": if self.utf8 { "utf-8" } else { "utf-16" },
                    "textDocumentSync": {"openClose": true, "change": 2, "save": true},
                    "completionProvider": {"triggerCharacters": ["."], "resolveProvider": true},
                    "hoverProvider": true,
                    "definitionProvider": true,
                    "referencesProvider": true,
                    "renameProvider": true,
                    "documentFormattingProvider": true,
                    "signatureHelpProvider": {"triggerCharacters": ["("]},
                }}));
            }
            "shutdown" => reply(Value::Null),
            "exit" => return false,
            "textDocument/didOpen" => {
                let uri = p["textDocument"]["uri"].as_str().unwrap_or_default().to_owned();
                self.docs.insert(uri.clone(), p["textDocument"]["text"].as_str().unwrap_or_default().to_owned());
                self.changed(&uri, p["textDocument"]["version"].as_i64());
            }
            "textDocument/didChange" => {
                let uri = p["textDocument"]["uri"].as_str().unwrap_or_default().to_owned();
                for c in p["contentChanges"].as_array().into_iter().flatten() {
                    let text = c["text"].as_str().unwrap_or_default();
                    let doc = self.docs.entry(uri.clone()).or_default();
                    if c.get("range").is_some() {
                        let a = offset(doc, &c["range"]["start"], self.utf8);
                        let b = offset(doc, &c["range"]["end"], self.utf8);
                        doc.replace_range(a..b, text);
                    } else {
                        *doc = text.to_owned();
                    }
                }
                self.changed(&uri, p["textDocument"]["version"].as_i64());
            }
            "textDocument/didClose" => {
                self.docs.remove(p["textDocument"]["uri"].as_str().unwrap_or_default());
            }
            "textDocument/completion" => reply(json!({"isIncomplete": false, "items": self.completions(p)})),
            "completionItem/resolve" => {
                let mut item = p.clone();
                item["documentation"] = json!({"kind": "markdown", "value": format!("Docs for `{}`", item["label"].as_str().unwrap_or_default())});
                if item["label"] == "fake_alpha" {
                    item["additionalTextEdits"] = json!([{"range": range_json(0, 0, 0, 0), "newText": "use fake;\n"}]);
                }
                reply(item);
            }
            "textDocument/hover" => {
                let (uri, word) = self.word_at(p);
                let result = match word {
                    Some((w, _)) if !w.is_empty() => json!({"contents": {"kind": "markdown", "value": format!("```rust\nfn {w}()\n```\nHover for `{w}` in {}", uri.rsplit('/').next().unwrap_or_default())}}),
                    _ => Value::Null,
                };
                reply(result);
            }
            "textDocument/definition" => {
                let (uri, word) = self.word_at(p);
                let found = word.and_then(|(w, _)| {
                    let doc = &self.docs[&uri];
                    let at = find_words(doc, &w).into_iter().next()?;
                    Some(json!({"uri": uri, "range": self.range_of(doc, at, at + w.len())}))
                });
                reply(found.unwrap_or(Value::Null));
            }
            "textDocument/references" => {
                let (_, word) = self.word_at(p);
                let mut out = Vec::new();
                if let Some((w, _)) = word {
                    for (uri, doc) in &self.docs {
                        for at in find_words(doc, &w) {
                            out.push(json!({"uri": uri, "range": self.range_of(doc, at, at + w.len())}));
                        }
                    }
                }
                reply(Value::Array(out));
            }
            "textDocument/rename" => {
                let (uri, word) = self.word_at(p);
                let new_name = p["newName"].as_str().unwrap_or_default();
                let Some((w, _)) = word else {
                    send(&json!({"jsonrpc": "2.0", "id": m["id"], "error": {"code": -32602, "message": "nothing to rename here"}}));
                    return true;
                };
                let mut changes = serde_json::Map::new();
                let mut texts: Vec<(String, String)> = self.docs.iter().map(|(u, t)| (u.clone(), t.clone())).collect();
                // Files of the same type in the project that aren't open: from disk.
                let ext = uri.rsplit('.').next().unwrap_or_default().to_owned();
                if let Some(root) = &self.root
                    && let Ok(dir) = std::fs::read_dir(root)
                {
                    for e in dir.flatten() {
                        let path = e.path();
                        let u = super::uri::from_path(&path);
                        if path.extension().is_some_and(|x| x.to_string_lossy() == ext) && !self.docs.contains_key(&u)
                            && let Ok(t) = std::fs::read_to_string(&path)
                        {
                            texts.push((u, t));
                        }
                    }
                }
                for (u, t) in texts {
                    let edits: Vec<Value> = find_words(&t, &w).into_iter().map(|at| json!({"range": self.range_of(&t, at, at + w.len()), "newText": new_name})).collect();
                    if !edits.is_empty() {
                        changes.insert(u, Value::Array(edits));
                    }
                }
                reply(json!({"changes": changes}));
            }
            "textDocument/formatting" => {
                let uri = p["textDocument"]["uri"].as_str().unwrap_or_default();
                let doc = self.docs.get(uri).cloned().unwrap_or_default();
                let mut edits = Vec::new();
                for (i, line) in doc.split('\n').enumerate() {
                    let body = line.strip_suffix('\r').unwrap_or(line);
                    let kept = body.trim_end_matches([' ', '\t']);
                    if kept.len() < body.len() {
                        let (a, b) = (self.col(kept), self.col(body));
                        edits.push(json!({"range": range_json(i, a, i, b), "newText": ""}));
                    }
                }
                reply(Value::Array(edits));
            }
            "textDocument/signatureHelp" => reply(json!({"signatures": [{"label": "fake_alpha(first: i32, second: &str)", "parameters": [{"label": "first: i32"}, {"label": "second: &str"}]}], "activeSignature": 0, "activeParameter": 0})),
            _ if m.get("id").is_some() && !method.is_empty() => reply(Value::Null),
            _ => {}
        }
        true
    }

    /// A document changed: maybe crash, then publish its diagnostics.
    fn changed(&mut self, uri: &str, version: Option<i64>) {
        let doc = self.docs[uri].clone();
        if doc.contains("CRASH_NOW")
            && let Some(marker) = &self.marker
            && !marker.exists()
        {
            let _ = std::fs::write(marker, "crashed");
            std::process::exit(3);
        }
        let mut diags = Vec::new();
        for (word, severity, message) in [("bad", 1, "“bad” is not allowed"), ("meh", 2, "“meh” could be better")] {
            for at in find_words(&doc, word) {
                diags.push(json!({"range": self.range_of(&doc, at, at + word.len()), "severity": severity, "message": message, "source": "fake"}));
            }
        }
        send(&json!({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": {"uri": uri, "version": version, "diagnostics": diags}}));
    }

    fn completions(&self, p: &Value) -> Vec<Value> {
        let (_, word) = self.word_at(p);
        let (start, end) = word.map(|(_, r)| r).unwrap_or((p["position"].clone(), p["position"].clone()));
        vec![
            json!({"label": "fake_alpha", "kind": 3, "detail": "fn fake_alpha(first: i32, second: &str, third: Vec<String>) -> Result<(), Error>", "sortText": "a"}),
            json!({"label": "fake_beta", "kind": 6, "insertText": "fake_beta", "sortText": "b"}),
            json!({"label": "fake_snippet", "kind": 15, "sortText": "c", "insertTextFormat": 2, "textEdit": {"range": {"start": start, "end": end}, "newText": "fake_snippet(${1:arg})$0"}}),
        ]
    }

    /// The document of a request, and the word around its position with that word's range.
    fn word_at(&self, p: &Value) -> (String, Option<Word>) {
        let uri = p["textDocument"]["uri"].as_str().unwrap_or_default().to_owned();
        let Some(doc) = self.docs.get(&uri) else { return (uri, None) };
        let at = offset(doc, &p["position"], self.utf8);
        let start = doc[..at].char_indices().rev().take_while(|(_, c)| word_char(*c)).last().map_or(at, |(i, _)| i);
        let end = at + doc[at..].char_indices().take_while(|(_, c)| word_char(*c)).last().map_or(0, |(i, c)| i + c.len_utf8());
        let w = doc[start..end].to_owned();
        let r = self.range_of(doc, start, end);
        (uri, Some((w, (r["start"].clone(), r["end"].clone()))))
    }

    /// Columns in the agreed units for the end of `s` (a line's beginning).
    fn col(&self, s: &str) -> usize {
        if self.utf8 { s.len() } else { s.encode_utf16().count() }
    }

    fn pos_of(&self, doc: &str, at: usize) -> (usize, usize) {
        let before = &doc[..at];
        let line = before.matches('\n').count();
        let line_start = before.rfind('\n').map_or(0, |i| i + 1);
        (line, self.col(&doc[line_start..at]))
    }

    fn range_of(&self, doc: &str, a: usize, b: usize) -> Value {
        let ((l0, c0), (l1, c1)) = (self.pos_of(doc, a), self.pos_of(doc, b));
        range_json(l0, c0, l1, c1)
    }
}

fn range_json(l0: usize, c0: usize, l1: usize, c1: usize) -> Value {
    json!({"start": {"line": l0, "character": c0}, "end": {"line": l1, "character": c1}})
}

/// Byte offset of a protocol position in `doc` (lines split at `\n`).
fn offset(doc: &str, pos: &Value, utf8: bool) -> usize {
    let (line, col) = (pos["line"].as_u64().unwrap_or(0) as usize, pos["character"].as_u64().unwrap_or(0) as usize);
    let mut start = 0;
    for _ in 0..line {
        match doc[start..].find('\n') {
            Some(i) => start += i + 1,
            None => return doc.len(),
        }
    }
    let end = doc[start..].find('\n').map_or(doc.len(), |i| start + i);
    let text = doc[start..end].strip_suffix('\r').unwrap_or(&doc[start..end]);
    let mut units = 0;
    for (i, c) in text.char_indices() {
        if units >= col {
            return start + i;
        }
        units += if utf8 { c.len_utf8() } else { c.len_utf16() };
    }
    start + text.len()
}

/// Byte offsets of `word` standing alone in `doc`.
fn find_words(doc: &str, word: &str) -> Vec<usize> {
    if word.is_empty() {
        return Vec::new();
    }
    doc.match_indices(word)
        .filter(|(i, _)| !doc[..*i].chars().next_back().is_some_and(word_char) && !doc[i + word.len()..].chars().next().is_some_and(word_char))
        .map(|(i, _)| i)
        .collect()
}
