//! A plugin for tests, so they need no Python or Node. The test binary runs
//! itself as the plugin: `launch` starts it with `STRATA_FAKE_PLUGIN` set,
//! and the `fake_plugin` test then speaks the protocol on stdin/stdout
//! instead of returning at once. A manifest whose `run.command` is
//! `COMMAND` starts it (tests only; see `manager`).
//!
//! `STRATA_FAKE_PLUGIN` holds words that change how it starts: `silent`
//! never answers anything, `refuse` answers `initialize` with an error,
//! `exit-at-start` exits at once, `bad-api` claims another API version.
//!
//! Its commands (`command/execute`'s `command`) call Strata's API and
//! answer with what Strata said: `read`, `upper` (upper-cases the document
//! in one edit), `stale` (an edit for an old version), `notify`, `status`,
//! `panel`, `pick`, `call` (any method, from `args`), `events` (the
//! notifications it got), `init` (its `initialize` parameters), `docs`
//! (the `document` of every `command/execute` so far),
//! `settings`, `crash` (exits with status 3), `hang` (never answers),
//! `burst` (`args.count` requests of `args.method` at once, then waits for
//! every answer),
//! `flood` (`args.count` log notifications, then some `ui/notify` ones),
//! `spawn-child` (starts `sleep 60` and says its pid).

use super::host::Launch;
use crate::lsp::jsonrpc;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::path::Path;

/// `run.command` that means "the fake plugin".
pub const COMMAND: &str = "strata-test-fake-plugin";

/// How to start the fake plugin with these words (see the module).
pub fn launch(words: &[String], dir: &Path) -> Launch {
    let exe = std::env::current_exe().expect("the test binary's path");
    Launch {
        program: exe,
        args: ["--exact", "plugins::fake::fake_plugin", "--nocapture", "--test-threads=1", "-q"].iter().map(|s| s.to_string()).collect(),
        cwd: dir.to_owned(),
        env: vec![("STRATA_FAKE_PLUGIN".into(), words.join(" ").into())],
    }
}

#[test]
fn fake_plugin() {
    // In a normal test run this does nothing.
    let Ok(words) = std::env::var("STRATA_FAKE_PLUGIN") else { return };
    let words: Vec<&str> = words.split_whitespace().collect();
    if words.contains(&"exit-at-start") {
        std::process::exit(2);
    }
    Fake { words: words.iter().map(|s| s.to_string()).collect(), ..Fake::default() }.serve();
    std::process::exit(0);
}

#[derive(Default)]
struct Fake {
    words: Vec<String>,
    decoder: jsonrpc::Decoder,
    queue: VecDeque<Value>,
    next_id: i64,
    init: Value,
    settings: Value,
    events: Vec<Value>,
    /// Each `command/execute`'s `document`, as it came.
    docs: Vec<Value>,
}

fn send(msg: &Value) {
    let body = msg.to_string();
    let mut out = std::io::stdout().lock();
    let _ = write!(out, "Content-Length: {}\r\n\r\n{body}", body.len());
    let _ = out.flush();
}

impl Fake {
    /// The next message from Strata (None at the end of input).
    fn next(&mut self) -> Option<Value> {
        let mut buf = [0u8; 65536];
        loop {
            if let Some(m) = self.queue.pop_front() {
                return Some(m);
            }
            while let Some(m) = self.decoder.next() {
                if let Ok(m) = m {
                    self.queue.push_back(m);
                }
            }
            if !self.queue.is_empty() {
                continue;
            }
            let n = match std::io::stdin().lock().read(&mut buf) {
                Ok(0) | Err(_) => return None,
                Ok(n) => n,
            };
            self.decoder.push(&buf[..n]);
        }
    }

    fn serve(&mut self) {
        let silent = self.words.iter().any(|w| w == "silent");
        while let Some(m) = self.next() {
            if silent {
                continue;
            }
            if !self.handle(m) {
                return;
            }
        }
    }

    /// False to stop.
    fn handle(&mut self, m: Value) -> bool {
        let method = m["method"].as_str().unwrap_or_default().to_owned();
        let id = m.get("id").cloned();
        let reply = |result: Value| send(&json!({"jsonrpc": "2.0", "id": id, "result": result}));
        match method.as_str() {
            "initialize" => {
                if self.words.iter().any(|w| w == "refuse") {
                    send(&json!({"jsonrpc": "2.0", "id": m["id"], "error": {"code": -32603, "message": "no thanks"}}));
                    return true;
                }
                self.init = m["params"].clone();
                self.settings = m["params"]["settings"].clone();
                let api = if self.words.iter().any(|w| w == "bad-api") { "strata.plugin/9" } else { super::API };
                reply(json!({"api": api}));
            }
            "shutdown" => reply(Value::Null),
            "exit" => return false,
            "command/execute" => {
                let cmd = m["params"]["command"].as_str().unwrap_or_default().to_owned();
                self.docs.push(m["params"]["document"].clone());
                let doc = m["params"]["document"]["id"].clone();
                let args = m["params"]["args"].clone();
                match self.command(&cmd, doc, args) {
                    Some(Ok(v)) => reply(v),
                    Some(Err(e)) => send(&json!({"jsonrpc": "2.0", "id": m["id"], "error": {"code": -32000, "message": e}})),
                    None => {}
                }
            }
            "" => {}
            _ if id.is_none() => {
                if method == "settings/changed" {
                    self.settings = m["params"]["settings"].clone();
                }
                self.events.push(json!({"method": method, "params": m["params"]}));
            }
            _ => send(&json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "unknown"}})),
        }
        true
    }

    /// Ask Strata something and wait for the answer (handling what else
    /// arrives meanwhile): `{"result": …}` or `{"error": …}`.
    fn call(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id + 1000;
        send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        while let Some(m) = self.next() {
            if m.get("method").is_none() && m["id"] == json!(id) {
                return match m.get("error") {
                    Some(e) => json!({"error": e}),
                    None => json!({"result": m["result"]}),
                };
            }
            if !self.handle(m) {
                std::process::exit(0);
            }
        }
        std::process::exit(0);
    }

    fn command(&mut self, cmd: &str, doc: Value, args: Value) -> Option<Result<Value, String>> {
        let doc_param = if doc.is_null() { json!({}) } else { json!({"document": doc}) };
        Some(Ok(match cmd {
            "call" => {
                if args["notify"].as_bool() == Some(true) {
                    send(&json!({"jsonrpc": "2.0", "method": args["method"], "params": args["params"]}));
                    return Some(Ok(json!("sent")));
                }
                let method = args["method"].as_str().unwrap_or_default().to_owned();
                self.call(&method, args["params"].clone())
            }
            "read" => self.call("document/read", doc_param),
            "upper" | "stale" => {
                let read = self.call("document/read", doc_param);
                let Some(text) = read["result"]["text"].as_str() else { return Some(Err(format!("couldn't read: {read}"))) };
                let d = &read["result"]["document"];
                let mut version = d["version"].as_u64().unwrap_or(0);
                if cmd == "stale" {
                    version = version.wrapping_sub(1);
                }
                let lines = text.split('\n').count() - 1;
                let last = text.rsplit('\n').next().unwrap_or("").chars().count();
                let edit = json!({"range": {"start": {"line": 0, "character": 0}, "end": {"line": lines, "character": last}}, "newText": text.to_uppercase()});
                self.call("document/applyEdits", json!({"document": d["id"], "version": version, "edits": [edit]}))
            }
            "notify" => self.call("ui/notify", json!({"message": "Hello from the fake plugin", "level": "info"})),
            "status" => self.call("ui/statusItem", json!({"id": "words", "text": "Fake: 3 words", "tooltip": "From the fake plugin"})),
            "panel" => self.call("ui/showPanel", json!({"title": "Fake Panel", "markdown": "# Hello\n\nFrom the **fake** plugin."})),
            "pick" => self.call("ui/pick", json!({"placeholder": "Pick one", "items": ["one", {"label": "two", "detail": "the second"}, "three"]})),
            "events" => json!(self.events),
            "init" => self.init.clone(),
            "docs" => json!(self.docs),
            "settings" => self.settings.clone(),
            "crash" => std::process::exit(3),
            "hang" => return None,
            "flood" => {
                let n = args["count"].as_u64().unwrap_or(20_000);
                for i in 0..n {
                    send(&json!({"jsonrpc": "2.0", "method": "log", "params": {"message": format!("flood {i}")}}));
                }
                for i in 0..200 {
                    send(&json!({"jsonrpc": "2.0", "method": "ui/notify", "params": {"message": format!("note {i}")}}));
                }
                json!("flooded")
            }
            "burst" => {
                let n = args["count"].as_u64().unwrap_or(1) as i64;
                let first = self.next_id + 1000 + 1;
                for k in 0..n {
                    send(&json!({"jsonrpc": "2.0", "id": first + k, "method": args["method"], "params": args["params"]}));
                }
                self.next_id += n;
                let (mut results, mut errors) = (0, 0);
                while results + errors < n {
                    let Some(m) = self.next() else { std::process::exit(0) };
                    let id = m["id"].as_i64().unwrap_or(-1);
                    if m.get("method").is_none() && (first..first + n).contains(&id) {
                        if m.get("error").is_some() {
                            errors += 1;
                        } else {
                            results += 1;
                        }
                    } else if !self.handle(m) {
                        std::process::exit(0);
                    }
                }
                json!({"results": results, "errors": errors})
            }
            "spawn-child" => {
                let child = std::process::Command::new("sleep").arg("60").spawn().map_err(|e| e.to_string());
                match child {
                    Ok(c) => json!(c.id()),
                    Err(e) => return Some(Err(e)),
                }
            }
            other => return Some(Err(format!("the fake plugin has no command {other}"))),
        }))
    }
}
