//! The HTTP side against a fake server on 127.0.0.1 (std's TcpListener),
//! through the real curl: both streaming formats, HTTP errors, broken and
//! stalled streams, dropped connections and cancelling.

use super::*;
use crate::ai::{Endpoint, Job, Provider};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};

type Script = Arc<dyn Fn(&mut TcpStream, &str) + Send + Sync>;

/// A server that answers every connection with `script`, and remembers
/// how many came and what they asked.
pub(crate) struct FakeServer {
    pub port: u16,
    pub hits: Arc<AtomicUsize>,
    pub requests: Arc<Mutex<Vec<String>>>,
}

impl FakeServer {
    pub fn start(script: impl Fn(&mut TcpStream, &str) + Send + Sync + 'static) -> FakeServer {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let hits = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let script: Script = Arc::new(script);
        let (h, r) = (hits.clone(), requests.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                h.fetch_add(1, Ordering::SeqCst);
                let (script, r) = (script.clone(), r.clone());
                std::thread::spawn(move || {
                    let req = read_request(&mut stream);
                    r.lock().unwrap().push(req.clone());
                    script(&mut stream, &req);
                });
            }
        });
        FakeServer { port, hits, requests }
    }

    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/v1", self.port)
    }

    pub fn provider(&self, kind: Kind) -> Provider {
        Provider {
            kind,
            endpoint: Endpoint::parse(&self.url()).unwrap(),
            model: "test-model".into(),
            stall: Duration::from_secs(2),
            remote_for_tests: false,
            program_for_tests: None,
        }
    }
}

fn read_request(s: &mut TcpStream) -> String {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    while !buf.ends_with(b"\r\n\r\n") {
        match s.read(&mut byte) {
            Ok(1) => buf.push(byte[0]),
            _ => break,
        }
    }
    let head = String::from_utf8_lossy(&buf).into_owned();
    let len = head.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0))).unwrap_or(0);
    let mut body = vec![0u8; len];
    let _ = s.read_exact(&mut body);
    head + &String::from_utf8_lossy(&body)
}

pub(crate) fn sse_headers(s: &mut TcpStream) {
    let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n");
}

pub(crate) fn send_slowly(s: &mut TcpStream, parts: &[&str]) {
    for p in parts {
        let _ = s.write_all(p.as_bytes());
        let _ = s.flush();
        std::thread::sleep(Duration::from_millis(20));
    }
}

pub(crate) fn openai_chunk(text: &str) -> String {
    format!("data: {}\n\n", serde_json::json!({"choices": [{"index": 0, "delta": {"content": text}, "finish_reason": null}]}))
}

pub(crate) fn openai_ok(s: &mut TcpStream, _req: &str) {
    sse_headers(s);
    send_slowly(s, &[": keep-alive\n\n", &openai_chunk("Hello"), &openai_chunk(", wörld"), "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n", "data: [DONE]\n\n"]);
}

fn anthropic_ok(s: &mut TcpStream, _req: &str) {
    sse_headers(s);
    send_slowly(
        s,
        &[
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\"content\":[]}}\n\n",
            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            "event: ping\ndata: {\"type\":\"ping\"}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hi \"}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"there\"}}\n\n",
            "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"max_tokens\"}}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        ],
    );
}

fn status(code: u16, body: &'static str) -> impl Fn(&mut TcpStream, &str) + Send + Sync {
    move |s, _| {
        let _ = write!(s, "HTTP/1.1 {code} Something\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    }
}

/// Run a chat on this thread and collect what it reported.
fn chat_events(p: &Provider, key: Option<&str>, cancel: &AtomicBool) -> Vec<Event> {
    let mut out = Vec::new();
    chat(p, key, &Prompt { system: "sys".into(), user: "Explain ünïcode \"quotes\" and \\ backslashes\nnew line".into() }, cancel, &mut |e| out.push(e));
    out
}

fn text(events: &[Event]) -> String {
    events.iter().filter_map(|e| if let Event::Text(t) = e { Some(t.as_str()) } else { None }).collect()
}

fn last(events: &[Event]) -> &Event {
    events.last().unwrap()
}

fn no() -> AtomicBool {
    AtomicBool::new(false)
}

#[test]
fn openai_stream_and_what_was_sent() {
    let srv = FakeServer::start(openai_ok);
    let ev = chat_events(&srv.provider(Kind::OpenAi), Some("sk-local-123"), &no());
    assert_eq!(text(&ev), "Hello, wörld");
    assert_eq!(last(&ev), &Event::Done(None));
    let req = srv.requests.lock().unwrap()[0].clone();
    assert!(req.starts_with("POST /v1/chat/completions HTTP/1.1\r\n"), "{req}");
    assert!(req.contains("Authorization: Bearer sk-local-123\r\n"));
    assert!(req.contains("Content-Type: application/json\r\n"));
    assert!(!req.to_ascii_lowercase().contains("expect: 100"), "no 100-continue pause");
    let body: Value = serde_json::from_str(&req[req.find("\r\n\r\n").unwrap() + 4..]).unwrap();
    assert_eq!(body["model"], "test-model");
    assert_eq!(body["stream"], true);
    assert_eq!(body["messages"][1]["content"], "Explain ünïcode \"quotes\" and \\ backslashes\nnew line", "the text arrives exactly");
}

#[test]
fn anthropic_stream() {
    let srv = FakeServer::start(anthropic_ok);
    let mut p = srv.provider(Kind::Anthropic);
    p.endpoint = Endpoint::parse(&format!("http://127.0.0.1:{}", srv.port)).unwrap();
    let ev = chat_events(&p, Some("sk-ant-test"), &no());
    assert_eq!(text(&ev), "Hi there");
    assert_eq!(last(&ev), &Event::Done(Some("The answer was cut off at the length limit.".into())));
    let req = srv.requests.lock().unwrap()[0].clone();
    assert!(req.starts_with("POST /v1/messages HTTP/1.1\r\n"), "{req}");
    assert!(req.contains("x-api-key: sk-ant-test\r\n") && req.contains("anthropic-version: 2023-06-01\r\n"));
    let body: Value = serde_json::from_str(&req[req.find("\r\n\r\n").unwrap() + 4..]).unwrap();
    assert_eq!(body["system"], "sys");
    assert_eq!(body["max_tokens"], crate::ai::MAX_ANSWER_TOKENS);
}

#[test]
fn anthropic_error_event_mid_stream() {
    let srv = FakeServer::start(|s, _| {
        sse_headers(s);
        send_slowly(s, &["event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Par\"}}\n\n", "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n\n"]);
    });
    let mut p = srv.provider(Kind::Anthropic);
    p.endpoint = Endpoint::parse(&format!("http://127.0.0.1:{}", srv.port)).unwrap();
    let ev = chat_events(&p, Some("k"), &no());
    assert_eq!(text(&ev), "Par");
    assert!(matches!(last(&ev), Event::Failed(m) if m.contains("overloaded") && m.contains("Overloaded")), "{ev:?}");
}

#[test]
fn http_errors_read_well() {
    let cases: [(u16, &'static str, Option<&str>, &str); 5] = [
        (401, r#"{"error":{"message":"Incorrect API key provided","type":"invalid_request_error"}}"#, Some("bad"), "didn't accept the API key (401): Incorrect API key provided"),
        (401, r#"{"type":"error","error":{"type":"authentication_error","message":"x-api-key header is required"}}"#, None, "needs an API key (401): x-api-key header is required"),
        (429, r#"{"error":{"message":"Rate limit reached"}}"#, None, "limiting requests (429): Rate limit reached"),
        (500, r#"{"error":"model runner crashed"}"#, None, "server error (500): model runner crashed"),
        (404, r#"{"error":{"message":"model 'nope' not found"}}"#, None, "(404): model 'nope' not found. Check the base URL"),
    ];
    for (code, body, key, want) in cases {
        let srv = FakeServer::start(status(code, body));
        let ev = chat_events(&srv.provider(Kind::OpenAi), key, &no());
        assert!(matches!(last(&ev), Event::Failed(m) if m.contains(want)), "{code}: {ev:?}");
        assert_eq!(text(&ev), "");
    }
}

#[test]
fn malformed_stream() {
    let srv = FakeServer::start(|s, _| {
        sse_headers(s);
        send_slowly(s, &[&openai_chunk("ok "), "data: {not json\n\n", &openai_chunk("never shown")]);
    });
    let ev = chat_events(&srv.provider(Kind::OpenAi), None, &no());
    assert_eq!(text(&ev), "ok ");
    assert!(matches!(last(&ev), Event::Failed(m) if m.contains("couldn't read")), "{ev:?}");
}

#[test]
fn a_whole_json_answer_is_fine_too() {
    let srv = FakeServer::start(status(200, r#"{"choices":[{"message":{"role":"assistant","content":"All at once"},"finish_reason":"stop"}]}"#));
    let ev = chat_events(&srv.provider(Kind::OpenAi), None, &no());
    assert_eq!((text(&ev).as_str(), last(&ev)), ("All at once", &Event::Done(None)));
}

#[test]
fn a_continue_before_the_answer() {
    let srv = FakeServer::start(|s, r| {
        let _ = s.write_all(b"HTTP/1.1 100 Continue\r\n\r\n");
        openai_ok(s, r);
    });
    let ev = chat_events(&srv.provider(Kind::OpenAi), None, &no());
    assert_eq!(text(&ev), "Hello, wörld");
}

#[test]
fn a_stalled_stream_is_given_up_on() {
    let srv = FakeServer::start(|s, _| {
        sse_headers(s);
        send_slowly(s, &[&openai_chunk("start")]);
        std::thread::sleep(Duration::from_secs(20));
    });
    let mut p = srv.provider(Kind::OpenAi);
    p.stall = Duration::from_secs(1);
    let t = Instant::now();
    let ev = chat_events(&p, None, &no());
    assert!(t.elapsed() < Duration::from_secs(8), "{:?}", t.elapsed());
    assert_eq!(text(&ev), "start");
    assert!(matches!(last(&ev), Event::Failed(m) if m.contains("sent nothing for 1 seconds")), "{ev:?}");
}

#[test]
fn a_server_that_never_answers() {
    // Accepts, reads the request, then says nothing at all.
    let srv = FakeServer::start(|_, _| std::thread::sleep(Duration::from_secs(20)));
    let mut p = srv.provider(Kind::OpenAi);
    p.stall = Duration::from_secs(1);
    let ev = chat_events(&p, None, &no());
    assert!(matches!(last(&ev), Event::Failed(m) if m.contains("sent nothing")), "{ev:?}");
}

#[test]
fn closed_early_and_dropped_connections() {
    // Closed cleanly before [DONE] or a finish reason.
    let srv = FakeServer::start(|s, _| {
        sse_headers(s);
        send_slowly(s, &[&openai_chunk("half")]);
    });
    let ev = chat_events(&srv.provider(Kind::OpenAi), None, &no());
    assert_eq!(text(&ev), "half");
    assert!(matches!(last(&ev), Event::Failed(m) if m.contains("closed before the answer finished")), "{ev:?}");
    // Dropped in the middle of a chunk.
    let srv = FakeServer::start(|s, _| {
        let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n");
        let c = openai_chunk("piece");
        let _ = write!(s, "{:x}\r\n{c}\r\n", c.len());
        let _ = s.write_all(b"400\r\ndata: {\"choices\"");
        let _ = s.flush();
        let _ = s.shutdown(std::net::Shutdown::Both);
    });
    let ev = chat_events(&srv.provider(Kind::OpenAi), None, &no());
    assert_eq!(text(&ev), "piece");
    assert!(matches!(last(&ev), Event::Failed(m) if m.contains("dropped")), "{ev:?}");
    // Nobody listening.
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let mut p = srv.provider(Kind::OpenAi);
    p.endpoint = Endpoint::parse(&format!("http://127.0.0.1:{port}/v1")).unwrap();
    let ev = chat_events(&p, None, &no());
    assert!(matches!(last(&ev), Event::Failed(m) if m.contains("Is the server running?")), "{ev:?}");
}

#[test]
fn cancel_stops_at_once_and_never_blocks() {
    let srv = FakeServer::start(|s, _| {
        sse_headers(s);
        send_slowly(s, &[&openai_chunk("first")]);
        std::thread::sleep(Duration::from_secs(20));
    });
    let mut p = srv.provider(Kind::OpenAi);
    p.stall = Duration::from_secs(30);
    let t = Instant::now();
    let job: Job = crate::ai::start(p, Prompt { system: "s".into(), user: "u".into() }, Arc::new(crate::ai::keychain::Memory::default()), None);
    assert!(t.elapsed() < Duration::from_millis(200), "starting returns at once");
    let first = job.rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(first, Event::Text("first".into()));
    let t = Instant::now();
    job.cancel();
    let end = job.rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(end, Event::Failed(crate::ai::CANCELLED.into()));
    assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
}

#[test]
fn test_connection_lists_models() {
    let srv = FakeServer::start(status(200, r#"{"object":"list","data":[{"id":"llama3.2"},{"id":"qwen2.5-coder"}]}"#));
    let mut p = srv.provider(Kind::OpenAi);
    assert_eq!(test(&p, None).unwrap(), format!("Connected to 127.0.0.1:{} (local): 2 models (llama3.2, qwen2.5-coder). The model \"test-model\" isn't among them.", srv.port));
    p.model = "llama3.2".into();
    assert!(test(&p, Some("k")).unwrap().ends_with("qwen2.5-coder)."));
    let req = srv.requests.lock().unwrap()[1].clone();
    assert!(req.starts_with("GET /v1/models HTTP/1.1\r\n") && req.contains("Authorization: Bearer k\r\n"), "{req}");
    let bad = FakeServer::start(status(401, r#"{"error":{"message":"bad key"}}"#));
    assert!(test(&bad.provider(Kind::OpenAi), Some("k")).unwrap_err().contains("didn't accept the API key (401): bad key"));
}

#[test]
fn config_is_quoted_and_nothing_is_on_the_command_line() {
    let req = Request {
        url: "https://h.example/v1/chat/completions".into(),
        headers: vec!["Authorization: Bearer sk-secret".into()],
        body: Some(ascii_json(&serde_json::json!({"x": "a\"b\\c\nd é 😀"}))),
        local: false,
        stall: Duration::from_secs(5),
        max_time: None,
    };
    let cfg = curl_config(&req).unwrap();
    assert!(cfg.is_ascii() && !cfg.contains('\t'));
    assert!(cfg.contains(r#"data-binary = "{\"x\":\"a\\\"b\\\\c\\nd \\u00e9 \\ud83d\\ude00\"}""#), "{cfg}");
    let args = curl_args(&req);
    assert_eq!(args[0], "-q");
    assert!(args.windows(2).any(|w| w == ["--proto", "=https"]));
    assert!(!args.iter().any(|a| a.contains("sk-secret") || a.contains("h.example")), "the key and address go on stdin");
    assert!(!args.iter().any(|a| a == "--location" || a == "-L"), "no redirects");
    let local = Request { local: true, ..req };
    let args = curl_args(&local);
    assert!(args.windows(2).any(|w| w == ["--noproxy", "*"]), "no proxy for local providers");
    // Headers can't smuggle a line break.
    let evil = Request { headers: vec!["X: a\r\nHost: evil".into()], ..local };
    assert!(curl_config(&evil).is_err());
    assert!(chat_request(&{
        let mut p = FakeServer { port: 1, hits: Default::default(), requests: Default::default() }.provider(Kind::OpenAi);
        p.model = "m".into();
        p
    }, Some("bad key\nX: y"), &Prompt { system: String::new(), user: String::new() })
    .is_err());
}
