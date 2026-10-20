//! Talking to OpenAI-compatible servers and the Anthropic API over HTTP,
//! through the system's curl (the same one the update check uses, see
//! `update::fetch`), with the answer streamed as server-sent events.
//!
//! Everything about a request (address, headers, key, body) goes to curl on
//! its standard input as a config file (`--config -`), so none of it shows
//! in the process list or touches the disk. curl ignores `~/.curlrc` (`-q`),
//! follows no redirects (a key or prompt never ends up somewhere else), uses
//! no proxy for local providers (so a proxy setting can't carry "local" text
//! off the machine), and only speaks https to remote ones.

use super::sse;
use super::{Event, Kind, Prompt, Provider};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::time::{Duration, Instant};

/// Answers bigger than this are stopped (a runaway stream).
const MAX_RESPONSE: usize = 8 << 20;
/// Error bodies kept for the message.
const MAX_ERROR_BODY: usize = 64 << 10;
/// Test Connection gives up after this long.
const TEST_TIME: Duration = Duration::from_secs(30);

pub struct Request {
    pub url: String,
    /// "Name: value", checked by `header_ok`.
    pub headers: Vec<String>,
    /// JSON to POST; None: GET.
    pub body: Option<String>,
    /// The provider is on this computer or the local network (no proxy, http allowed).
    pub local: bool,
    /// Give up when nothing arrives for this long.
    pub stall: Duration,
    /// Give up after this long in all (Test Connection).
    pub max_time: Option<Duration>,
}

#[derive(Debug, PartialEq)]
pub enum Failure {
    Cancelled,
    Stalled(Duration),
    /// An HTTP status other than 200, and the start of the body.
    Status(u16, String),
    /// curl failed (exit code, its last message).
    Curl(Option<i32>, String),
    Spawn(String),
    TooBig,
    /// Something Strata refused to send.
    Refused(String),
}

/// A header value can't carry a line break or other control characters
/// (they'd add headers of their own).
fn header_ok(h: &str) -> bool {
    h.contains(": ") && !h.chars().any(|c| c.is_control())
}

/// A key is printable ASCII without spaces, as every provider's are.
pub fn key_ok(key: &str) -> bool {
    !key.is_empty() && key.len() <= 4096 && key.bytes().all(|b| (0x21..=0x7e).contains(&b))
}

/// `s` as a quoted string in a curl config file.
fn quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// JSON with every non-ASCII character escaped, so the config curl reads is
/// plain ASCII whatever the system's code page.
pub fn ascii_json(v: &Value) -> String {
    let s = v.to_string();
    let mut out = String::with_capacity(s.len());
    let mut buf = [0u16; 2];
    for c in s.chars() {
        if c.is_ascii() {
            out.push(c);
        } else {
            for unit in c.encode_utf16(&mut buf) {
                out.push_str(&format!("\\u{unit:04x}"));
            }
        }
    }
    out
}

/// The config file curl reads from its standard input.
pub fn curl_config(req: &Request) -> Result<String, Failure> {
    if req.url.chars().any(|c| c.is_control() || c.is_whitespace() || c == '"') {
        return Err(Failure::Refused("the address has characters curl can't take".into()));
    }
    let mut cfg = format!("url = {}\n", quoted(&req.url));
    // No "Expect: 100-continue" pause before big bodies.
    cfg.push_str("header = \"Expect:\"\n");
    for h in &req.headers {
        if !header_ok(h) {
            return Err(Failure::Refused("a header has characters that can't be sent".into()));
        }
        cfg.push_str(&format!("header = {}\n", quoted(h)));
    }
    if let Some(body) = &req.body {
        if body.chars().any(|c| c.is_control() || !c.is_ascii()) {
            return Err(Failure::Refused("the request body isn't plain JSON".into()));
        }
        cfg.push_str("header = \"Content-Type: application/json\"\n");
        cfg.push_str(&format!("data-binary = {}\n", quoted(body)));
    }
    Ok(cfg)
}

/// curl's arguments (the request itself comes on standard input).
pub fn curl_args(req: &Request) -> Vec<String> {
    // -q must come first: don't read ~/.curlrc.
    let mut a: Vec<String> = ["-q", "--silent", "--show-error", "--no-buffer", "--include", "--suppress-connect-headers", "--connect-timeout", "20"].map(String::from).to_vec();
    a.extend(["--proto".into(), if req.local { "=http,https" } else { "=https" }.into()]);
    if req.local {
        a.extend(["--noproxy".into(), "*".into()]);
    }
    if let Some(t) = req.max_time {
        a.extend(["--max-time".into(), t.as_secs().max(1).to_string()]);
    }
    a.extend(["--user-agent".into(), format!("Strata/{}", env!("CARGO_PKG_VERSION"))]);
    a.extend(["--config".into(), "-".into()]);
    a
}

/// Run `req`. The body lines of a 200 answer go to `line` as they arrive;
/// it returns true once it has the whole answer (curl is then stopped).
pub fn send(req: &Request, cancel: &AtomicBool, line: &mut dyn FnMut(&str) -> bool) -> Result<(), Failure> {
    let config = curl_config(req)?;
    if cancel.load(Ordering::Relaxed) {
        return Err(Failure::Cancelled);
    }
    let curl = crate::update::fetch::curl_path();
    let mut cmd = Command::new(&curl);
    cmd.args(curl_args(req)).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    crate::update::fetch::no_window(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| {
        Failure::Spawn(if cfg!(windows) {
            format!("Couldn't run {} ({e}); Windows 10 version 1803 or later includes it", curl.display())
        } else {
            format!("Couldn't run {}: {e}", curl.display())
        })
    })?;
    // Write on a helper thread: curl may stop reading (an error) before the end.
    if let Some(mut stdin) = child.stdin.take() {
        std::thread::spawn(move || {
            let _ = stdin.write_all(config.as_bytes());
        });
    }
    let mut stderr = child.stderr.take();
    let errors = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(e) = stderr.as_mut() {
            let _ = e.take(16 * 1024).read_to_string(&mut s);
        }
        s
    });
    let (tx, rx) = channel::<Option<Vec<u8>>>();
    if let Some(stdout) = child.stdout.take() {
        std::thread::spawn(move || {
            let mut r = BufReader::new(stdout);
            loop {
                let mut buf = Vec::new();
                match r.by_ref().take(MAX_RESPONSE as u64 + 1).read_until(b'\n', &mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if tx.send(Some(buf)).is_err() {
                            return;
                        }
                    }
                }
            }
            let _ = tx.send(None);
        });
    }
    let stop = |child: &mut std::process::Child| {
        let _ = child.kill();
        let _ = child.wait();
    };
    let mut status: Option<u16> = None;
    let mut in_body = false;
    let mut error_body = String::new();
    let mut total = 0usize;
    let mut last = Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) {
            stop(&mut child);
            return Err(Failure::Cancelled);
        }
        let got = match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(Some(bytes)) => bytes,
            Ok(None) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {
                if last.elapsed() > req.stall {
                    stop(&mut child);
                    return Err(Failure::Stalled(req.stall));
                }
                continue;
            }
        };
        last = Instant::now();
        total += got.len();
        if total > MAX_RESPONSE {
            stop(&mut child);
            return Err(Failure::TooBig);
        }
        let text = String::from_utf8_lossy(&got);
        let text = text.trim_end_matches(['\n', '\r']);
        if !in_body {
            if text.starts_with("HTTP/") {
                status = text.split_whitespace().nth(1).and_then(|c| c.parse().ok());
            } else if text.is_empty() && status.is_some() {
                // 1xx answers (100 Continue) come before the real one.
                if status.is_some_and(|s| (100..200).contains(&s)) {
                    status = None;
                } else {
                    in_body = true;
                }
            }
            continue;
        }
        if status == Some(200) {
            if line(text) {
                stop(&mut child);
                return Ok(());
            }
        } else if error_body.len() < MAX_ERROR_BODY {
            error_body.push_str(text);
            error_body.push('\n');
        }
    }
    let exit = child.wait().ok();
    let stderr = errors.join().unwrap_or_default();
    let detail = stderr.lines().last().unwrap_or("").trim_start_matches("curl: ").trim().to_owned();
    match status {
        Some(200) if exit.is_some_and(|e| e.success()) => Ok(()),
        Some(200) => Err(Failure::Curl(exit.and_then(|e| e.code()), detail)),
        Some(s) => Err(Failure::Status(s, error_body)),
        None if exit.is_some_and(|e| e.success()) => Err(Failure::Curl(Some(52), detail)),
        None => Err(Failure::Curl(exit.and_then(|e| e.code()), detail)),
    }
}

/// The message in an error body, from either API's shape (or Ollama's).
fn error_message(body: &str) -> Option<String> {
    let v: Value = serde_json::from_str(body.trim()).ok()?;
    let m = v.pointer("/error/message").or_else(|| v.get("error").filter(|e| e.is_string())).or_else(|| v.get("message")).or_else(|| v.get("detail"))?;
    let m = m.as_str()?;
    Some(clean(m))
}

/// A server's text for a message: one line, at most 300 characters.
fn clean(s: &str) -> String {
    let s: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() > 300 { format!("{}…", s.chars().take(300).collect::<String>()) } else { s }
}

/// A failure as something a person can act on.
pub fn explain(f: &Failure, host: &str, local: bool, had_key: bool) -> String {
    match f {
        Failure::Cancelled => super::CANCELLED.into(),
        Failure::Stalled(d) => format!("{host} sent nothing for {} seconds, so Strata stopped waiting (the ai_timeout_seconds setting).", d.as_secs()),
        Failure::TooBig => "The answer was far too long, so Strata stopped it.".into(),
        Failure::Spawn(e) | Failure::Refused(e) => e.clone(),
        Failure::Status(status, body) => {
            let msg = error_message(body).map_or(String::new(), |m| format!(": {m}"));
            match status {
                401 if !had_key => format!("{host} needs an API key (401){msg}. Add one in Settings ▸ AI."),
                401 => format!("{host} didn't accept the API key (401){msg}. Check it in Settings ▸ AI."),
                403 => format!("{host} refused the request (403){msg}."),
                404 => format!("{host} has no such address or model (404){msg}. Check the base URL and the model name."),
                408 | 504 => format!("{host} timed out ({status}){msg}."),
                413 => format!("The request is too big for {host} (413){msg}. Lower ai_max_prompt_kb."),
                429 => format!("{host} is limiting requests (429){msg}. Wait a moment and try again."),
                503 | 529 => format!("{host} is overloaded or unavailable ({status}){msg}. Try again shortly."),
                s if *s >= 500 => format!("{host} had a server error ({s}){msg}."),
                s => format!("{host} rejected the request ({s}){msg}."),
            }
        }
        Failure::Curl(code, detail) => {
            let detail = clean(detail);
            match code {
                Some(5 | 6) => format!("Couldn't find {host}. Check the address and your connection."),
                Some(7) if local => format!("Couldn't connect to {host}. Is the server running?"),
                Some(7) => format!("Couldn't connect to {host}. Are you online?"),
                Some(28) => format!("{host} took too long to answer."),
                Some(35 | 51 | 53 | 54 | 58 | 59 | 60 | 66 | 77 | 80 | 82 | 83 | 90 | 91) => format!("Couldn't make a secure connection to {host} ({detail})."),
                Some(52) => format!("{host} closed the connection without answering."),
                Some(18 | 55 | 56) => format!("The connection to {host} dropped before the answer finished."),
                Some(c) => format!("The request to {host} failed (curl exit {c}{}).", if detail.is_empty() { String::new() } else { format!(": {detail}") }),
                None => format!("The request to {host} was stopped."),
            }
        }
    }
}

/// The request for a chat with `provider`.
pub fn chat_request(provider: &Provider, key: Option<&str>, prompt: &Prompt) -> Result<Request, String> {
    let mut headers = Vec::new();
    if let Some(k) = key
        && !key_ok(k)
    {
        return Err("The saved API key has characters no key has: save it again in Settings ▸ AI.".into());
    }
    let (url, body) = match provider.kind {
        Kind::Anthropic => {
            if let Some(k) = key {
                headers.push(format!("x-api-key: {k}"));
            }
            headers.push(format!("anthropic-version: {}", super::ANTHROPIC_VERSION));
            let body = json!({
                "model": provider.model,
                "max_tokens": super::MAX_ANSWER_TOKENS,
                "stream": true,
                "system": prompt.system,
                "messages": [{"role": "user", "content": prompt.user}],
            });
            (provider.endpoint.url("/v1/messages"), body)
        }
        _ => {
            if let Some(k) = key {
                headers.push(format!("Authorization: Bearer {k}"));
            }
            let body = json!({
                "model": provider.model,
                "stream": true,
                "messages": [{"role": "system", "content": prompt.system}, {"role": "user", "content": prompt.user}],
            });
            (provider.endpoint.url("/chat/completions"), body)
        }
    };
    headers.push("Accept: text/event-stream".into());
    Ok(Request { url, headers, body: Some(ascii_json(&body)), local: provider.endpoint.is_local(), stall: provider.stall, max_time: None })
}

/// Reads an answer as it streams: either API's events, or a whole JSON
/// answer from a server that doesn't stream.
pub struct Decoder {
    kind: Kind,
    sse: sse::Parser,
    /// The body isn't events: a JSON answer, gathered to the end.
    whole: Option<String>,
    started: bool,
    pub done: bool,
    /// The answer stopped for a reason worth telling.
    pub note: Option<String>,
    pub error: Option<String>,
}

impl Decoder {
    pub fn new(kind: Kind) -> Self {
        Decoder { kind, sse: sse::Parser::default(), whole: None, started: false, done: false, note: None, error: None }
    }

    /// One body line; true when the answer is complete (or failed).
    pub fn line(&mut self, line: &str, emit: &mut dyn FnMut(Event)) -> bool {
        if !self.started {
            if line.trim().is_empty() {
                return false;
            }
            self.started = true;
            if line.trim_start().starts_with('{') {
                self.whole = Some(String::new());
            }
        }
        if let Some(w) = &mut self.whole {
            if w.len() < MAX_RESPONSE {
                w.push_str(line);
                w.push('\n');
            }
            return false;
        }
        match self.sse.line(line) {
            Some(m) => self.message(m, emit),
            None => false,
        }
    }

    /// The stream ended.
    pub fn end(&mut self, emit: &mut dyn FnMut(Event)) {
        if let Some(w) = self.whole.take() {
            return self.whole_answer(&w, emit);
        }
        if let Some(m) = self.sse.flush() {
            self.message(m, emit);
        }
    }

    fn unreadable(&mut self) -> bool {
        self.error = Some("The provider sent an answer Strata couldn't read (not the streaming format it expected).".into());
        true
    }

    fn message(&mut self, m: sse::Message, emit: &mut dyn FnMut(Event)) -> bool {
        if self.done || self.error.is_some() {
            return true;
        }
        if m.data.trim() == "[DONE]" {
            self.done = true;
            return true;
        }
        let Ok(v) = serde_json::from_str::<Value>(&m.data) else { return self.unreadable() };
        if m.event == "error" || v.get("error").is_some_and(|e| !e.is_null()) {
            let kind = v.pointer("/error/type").and_then(Value::as_str).unwrap_or("");
            let msg = error_message(&m.data).unwrap_or_else(|| "no details".into());
            self.error = Some(if kind == "overloaded_error" { format!("The provider is overloaded right now ({msg}). Try again shortly.") } else { format!("The provider reported an error: {msg}") });
            return true;
        }
        match self.kind {
            Kind::Anthropic => match v.get("type").and_then(Value::as_str).unwrap_or(&m.event) {
                "content_block_delta" => {
                    if v.pointer("/delta/type").and_then(Value::as_str) == Some("text_delta")
                        && let Some(t) = v.pointer("/delta/text").and_then(Value::as_str)
                        && !t.is_empty()
                    {
                        emit(Event::Text(t.to_owned()));
                    }
                }
                "message_delta" => match v.pointer("/delta/stop_reason").and_then(Value::as_str) {
                    Some("max_tokens") => self.note = Some("The answer was cut off at the length limit.".into()),
                    Some("refusal") => self.note = Some("The model declined to answer.".into()),
                    _ => {}
                },
                "message_stop" => {
                    self.done = true;
                    return true;
                }
                _ => {} // message_start, content_block_start/stop, ping
            },
            _ => {
                let Some(choice) = v.pointer("/choices/0") else {
                    // A usage-only chunk, or a server's own keep-alive.
                    return false;
                };
                if let Some(t) = choice.pointer("/delta/content").and_then(Value::as_str)
                    && !t.is_empty()
                {
                    emit(Event::Text(t.to_owned()));
                }
                match choice.get("finish_reason").and_then(Value::as_str) {
                    Some("length") => self.note = Some("The answer was cut off at the model's length limit.".into()),
                    Some("content_filter") => self.note = Some("The provider filtered the answer.".into()),
                    _ => {}
                }
                if choice.get("finish_reason").is_some_and(|f| !f.is_null()) {
                    // Some servers end here without [DONE]; a usage chunk may still follow.
                    self.done = true;
                }
            }
        }
        false
    }

    fn whole_answer(&mut self, body: &str, emit: &mut dyn FnMut(Event)) {
        let Ok(v) = serde_json::from_str::<Value>(body.trim()) else {
            self.unreadable();
            return;
        };
        if v.get("error").is_some_and(|e| !e.is_null()) {
            self.error = Some(format!("The provider reported an error: {}", error_message(body).unwrap_or_else(|| "no details".into())));
            return;
        }
        let text = match self.kind {
            Kind::Anthropic => v.get("content").and_then(Value::as_array).map(|blocks| blocks.iter().filter_map(|b| b.get("text").and_then(Value::as_str)).collect::<String>()),
            _ => v.pointer("/choices/0/message/content").and_then(Value::as_str).map(str::to_owned),
        };
        match text {
            Some(t) => {
                if !t.is_empty() {
                    emit(Event::Text(t));
                }
                self.done = true;
            }
            None => {
                self.unreadable();
            }
        }
    }
}

/// Send `prompt` and stream the answer to `emit`.
pub fn chat(provider: &Provider, key: Option<&str>, prompt: &Prompt, cancel: &AtomicBool, emit: &mut dyn FnMut(Event)) {
    let req = match chat_request(provider, key, prompt) {
        Ok(r) => r,
        Err(e) => return emit(Event::Failed(e)),
    };
    let host = provider.endpoint.host_label();
    let mut dec = Decoder::new(provider.kind);
    let result = send(&req, cancel, &mut |l| dec.line(l, emit));
    match result {
        Err(f) => {
            if !matches!(f, Failure::Cancelled) {
                log::warn!("AI request to {host} failed: {}", failure_kind(&f));
            }
            emit(Event::Failed(dec.error.take().unwrap_or_else(|| explain(&f, &host, req.local, key.is_some()))));
        }
        Ok(()) => {
            dec.end(emit);
            match (dec.error.take(), dec.done) {
                (Some(e), _) => emit(Event::Failed(e)),
                (None, true) => emit(Event::Done(dec.note.take())),
                (None, false) => emit(Event::Failed(format!("The connection to {host} closed before the answer finished."))),
            }
        }
    }
}

/// For the log: what went wrong, without anything the server said.
fn failure_kind(f: &Failure) -> String {
    match f {
        Failure::Cancelled => "cancelled".into(),
        Failure::Stalled(d) => format!("nothing for {}s", d.as_secs()),
        Failure::Status(s, _) => format!("HTTP {s}"),
        Failure::Curl(c, _) => format!("curl exit {c:?}"),
        Failure::Spawn(_) => "curl didn't start".into(),
        Failure::TooBig => "answer too long".into(),
        Failure::Refused(_) => "refused to send".into(),
    }
}

/// Test Connection: list the models (no user text is sent).
pub fn test(provider: &Provider, key: Option<&str>) -> super::TestResult {
    if let Some(k) = key
        && !key_ok(k)
    {
        return Err("The saved API key has characters no key has: save it again.".into());
    }
    let host = provider.endpoint.host_label();
    let mut headers = Vec::new();
    let url = match provider.kind {
        Kind::Anthropic => {
            if let Some(k) = key {
                headers.push(format!("x-api-key: {k}"));
            }
            headers.push(format!("anthropic-version: {}", super::ANTHROPIC_VERSION));
            provider.endpoint.url("/v1/models")
        }
        _ => {
            if let Some(k) = key {
                headers.push(format!("Authorization: Bearer {k}"));
            }
            provider.endpoint.url("/models")
        }
    };
    let req = Request { url, headers, body: None, local: provider.endpoint.is_local(), stall: TEST_TIME, max_time: Some(TEST_TIME) };
    let mut body = String::new();
    let never = AtomicBool::new(false);
    send(&req, &never, &mut |l| {
        if body.len() < MAX_RESPONSE {
            body.push_str(l);
            body.push('\n');
        }
        false
    })
    .map_err(|f| explain(&f, &host, req.local, key.is_some()))?;
    let ids: Vec<String> = serde_json::from_str::<Value>(body.trim())
        .ok()
        .and_then(|v| v.get("data").and_then(Value::as_array).map(|a| a.iter().filter_map(|m| m.get("id").and_then(Value::as_str).map(clean)).collect()))
        .unwrap_or_default();
    let place = if req.local { format!("{host} (local)") } else { host };
    if ids.is_empty() {
        return Ok(format!("Connected to {place}."));
    }
    let shown: Vec<&str> = ids.iter().take(8).map(String::as_str).collect();
    let more = if ids.len() > shown.len() { format!(", and {} more", ids.len() - shown.len()) } else { String::new() };
    let missing = if !provider.model.is_empty() && !ids.contains(&provider.model) { format!(" The model \"{}\" isn't among them.", provider.model) } else { String::new() };
    Ok(format!("Connected to {place}: {} model{} ({}{more}).{missing}", ids.len(), if ids.len() == 1 { "" } else { "s" }, shown.join(", ")))
}

#[cfg(test)]
pub(crate) mod tests;
