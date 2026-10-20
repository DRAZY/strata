//! AI helpers with a model the user chooses (issue #10): an OpenAI-compatible
//! endpoint (Ollama, LM Studio, OpenRouter, OpenAI…), the Anthropic API, or
//! the `claude` / `codex` command-line tools with the user's own login.
//!
//! The rules this module keeps:
//! - Nothing happens until `ai_enabled` is on and the user asks for a helper.
//! - A provider that isn't on this computer or the local network (`locality`)
//!   gets nothing until the user has seen the exact text and said yes
//!   (`consent`; the app shows the question).
//! - Keys live in the system keychain (`keychain`), bound to the origin they
//!   were saved for, and reach curl through its standard input, never the
//!   command line, a file or the log.
//! - Requests run on background threads and can always be cancelled.
//! - Prompts, answers and keys are never logged.
//!
//! HTTP goes through the system's curl, like the update check (see
//! `update::fetch`), so there's one network stack: `http`.

pub mod cli;
pub mod consent;
pub mod http;
pub mod keychain;
pub mod prompt;
pub mod sse;

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::Arc;
use std::time::Duration;

/// The Anthropic API (tests point a provider elsewhere).
pub const ANTHROPIC_URL: &str = "https://api.anthropic.com";
pub const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Room for an answer: summaries are short, but a long review shouldn't be cut off.
pub const MAX_ANSWER_TOKENS: u32 = 16_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    OpenAi,
    Anthropic,
    ClaudeCli,
    CodexCli,
}

impl Kind {
    pub fn parse(s: &str) -> Option<Kind> {
        match s.trim() {
            "openai" => Some(Kind::OpenAi),
            "anthropic" => Some(Kind::Anthropic),
            "claude_cli" => Some(Kind::ClaudeCli),
            "codex_cli" => Some(Kind::CodexCli),
            _ => None,
        }
    }

    pub fn is_cli(self) -> bool {
        matches!(self, Kind::ClaudeCli | Kind::CodexCli)
    }

    /// The program a command-line provider runs.
    pub fn program(self) -> &'static str {
        match self {
            Kind::CodexCli => "codex",
            _ => "claude",
        }
    }
}

/// A provider's address: scheme, host, port and path, checked so it can't
/// smuggle anything into curl's configuration or carry a password.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    pub https: bool,
    /// Lowercase; an IPv6 address without its brackets.
    pub host: String,
    pub port: Option<u16>,
    /// Without a trailing slash ("" or "/v1").
    pub path: String,
}

impl Endpoint {
    pub fn parse(url: &str) -> Result<Endpoint, String> {
        let url = url.trim();
        let bad = || format!("\"{url}\" isn't a web address Strata can use (like http://localhost:11434/v1)");
        if url.chars().any(|c| c.is_control() || c.is_whitespace() || matches!(c, '"' | '\\' | '<' | '>' | '`')) {
            return Err(bad());
        }
        let (https, rest) = if let Some(r) = strip_prefix_ci(url, "https://") {
            (true, r)
        } else if let Some(r) = strip_prefix_ci(url, "http://") {
            (false, r)
        } else {
            return Err(format!("\"{url}\" must start with http:// or https://"));
        };
        if rest.contains(['?', '#']) {
            return Err(bad());
        }
        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, ""),
        };
        if authority.contains('@') {
            return Err("Put the key in Settings ▸ AI, not in the address".into());
        }
        let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
            let (h, after) = v6.split_once(']').ok_or_else(bad)?;
            let port = match after {
                "" => None,
                p => Some(p.strip_prefix(':').ok_or_else(bad)?),
            };
            h.parse::<Ipv6Addr>().map_err(|_| bad())?;
            (h.to_ascii_lowercase(), port)
        } else {
            match authority.rsplit_once(':') {
                Some((h, p)) => (h.to_ascii_lowercase(), Some(p)),
                None => (authority.to_ascii_lowercase(), None),
            }
        };
        let port = match port {
            Some(p) => Some(p.parse::<u16>().ok().filter(|&p| p > 0).ok_or_else(bad)?),
            None => None,
        };
        let host_ok = !host.is_empty() && (host.parse::<Ipv6Addr>().is_ok() || host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')));
        if !host_ok {
            return Err(bad());
        }
        Ok(Endpoint { https, host, port, path: path.trim_end_matches('/').to_owned() })
    }

    /// "https://host:port" (the port only when given).
    pub fn origin(&self) -> String {
        let host = if self.host.contains(':') { format!("[{}]", self.host) } else { self.host.clone() };
        let port = self.port.map_or(String::new(), |p| format!(":{p}"));
        format!("{}://{host}{port}", if self.https { "https" } else { "http" })
    }

    /// The full address of `suffix` ("/chat/completions") under this endpoint.
    pub fn url(&self, suffix: &str) -> String {
        format!("{}{}{suffix}", self.origin(), self.path)
    }

    /// "host" or "host:port", for messages.
    pub fn host_label(&self) -> String {
        let host = if self.host.contains(':') { format!("[{}]", self.host) } else { self.host.clone() };
        match self.port {
            Some(p) => format!("{host}:{p}"),
            None => host,
        }
    }

    /// On this computer or the local network: see `is_local_host`.
    pub fn is_local(&self) -> bool {
        is_local_host(&self.host)
    }
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    (s.len() >= prefix.len() && s.is_char_boundary(prefix.len()) && s[..prefix.len()].eq_ignore_ascii_case(prefix)).then(|| &s[prefix.len()..])
}

/// Whether a host is on this computer or the local network, judged from the
/// address as written, never from what a name resolves to (that can change
/// between asking and connecting, and split DNS can make a cloud name look
/// private): `localhost`, mDNS names (`*.local`, only ever answered on the
/// local link), and literal loopback, private (10/8, 172.16/12, 192.168/16,
/// fc00::/7) and link-local (169.254/16, fe80::/10) addresses. Every other
/// name counts as remote, even one that happens to resolve to a private
/// address. curl answers `localhost` itself, without DNS.
pub fn is_local_host(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host == "localhost" || (host.ends_with(".local") && host.len() > ".local".len()) {
        return true;
    }
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(v4)) => local_v4(v4),
        Ok(IpAddr::V6(v6)) => match v6.to_ipv4_mapped() {
            Some(v4) => local_v4(v4),
            None => v6.is_loopback() || (v6.segments()[0] & 0xfe00) == 0xfc00 || (v6.segments()[0] & 0xffc0) == 0xfe80,
        },
        Err(_) => false,
    }
}

fn local_v4(ip: Ipv4Addr) -> bool {
    ip.is_loopback() || ip.is_private() || ip.is_link_local()
}

/// Where a request goes, resolved from the settings.
#[derive(Clone, Debug, PartialEq)]
pub struct Provider {
    pub kind: Kind,
    /// The OpenAI-compatible base URL, or the Anthropic API's origin. Unused
    /// by command-line providers.
    pub endpoint: Endpoint,
    /// Empty: the command-line tool's own default.
    pub model: String,
    /// Give up when nothing arrives for this long.
    pub stall: Duration,
    /// Tests: treat a local test server as a remote provider.
    #[cfg(test)]
    pub remote_for_tests: bool,
    /// Tests: the program a command-line provider runs.
    #[cfg(test)]
    pub program_for_tests: Option<std::path::PathBuf>,
}

impl Provider {
    pub fn from_settings(s: &crate::session::Settings) -> Result<Provider, String> {
        let kind = Kind::parse(&s.ai_provider).ok_or_else(|| format!("Unknown AI provider \"{}\": choose one in Settings ▸ AI", s.ai_provider))?;
        let (endpoint, model) = match kind {
            Kind::OpenAi => {
                let e = Endpoint::parse(&s.ai_base_url)?;
                if !e.https && !e.is_local() {
                    return Err(format!("{} isn't on this computer or your local network, so it needs https:// (keys and code would otherwise cross the network unencrypted)", e.host_label()));
                }
                if s.ai_model.trim().is_empty() {
                    return Err("Set the model to use in Settings ▸ AI (Test Connection lists the ones the server has)".into());
                }
                (e, s.ai_model.trim().to_owned())
            }
            Kind::Anthropic => {
                if s.ai_anthropic_model.trim().is_empty() {
                    return Err("Set the Anthropic model in Settings ▸ AI".into());
                }
                (Endpoint::parse(ANTHROPIC_URL).expect("valid"), s.ai_anthropic_model.trim().to_owned())
            }
            Kind::ClaudeCli | Kind::CodexCli => (Endpoint::parse("http://localhost").expect("valid"), s.ai_cli_model.trim().to_owned()),
        };
        // The model goes on a command line (CLI tools) and into JSON: keep it plain.
        if !model.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ':' | '/' | '@' | '+')) || model.len() > 200 {
            return Err(format!("The model name \"{model}\" has characters Strata won't pass on"));
        }
        let stall = Duration::from_secs(u64::from(s.ai_timeout_seconds.clamp(5, 3600)));
        Ok(Provider {
            kind,
            endpoint,
            model,
            stall,
            #[cfg(test)]
            remote_for_tests: false,
            #[cfg(test)]
            program_for_tests: None,
        })
    }

    /// On this computer or the local network. Command-line tools send to
    /// their vendor, so they never are.
    pub fn is_local(&self) -> bool {
        #[cfg(test)]
        if self.remote_for_tests {
            return false;
        }
        !self.kind.is_cli() && self.kind != Kind::Anthropic && self.endpoint.is_local()
    }

    /// What consent is remembered under: a different host or tool asks again.
    pub fn consent_id(&self) -> String {
        match self.kind {
            Kind::OpenAi => format!("openai {}", self.endpoint.origin()),
            Kind::Anthropic => format!("anthropic {}", self.endpoint.origin()),
            Kind::ClaudeCli | Kind::CodexCli => format!("cli {}", self.kind.program()),
        }
    }

    /// The keychain entry for this provider's key. A key is saved for one
    /// origin and only ever sent there.
    pub fn key_account(&self) -> Option<String> {
        match self.kind {
            Kind::OpenAi => Some(format!("openai {}", self.endpoint.origin())),
            Kind::Anthropic => Some(format!("anthropic {}", self.endpoint.origin())),
            Kind::ClaudeCli | Kind::CodexCli => None,
        }
    }

    /// Where the text goes, for the consent question and the panel.
    pub fn destination(&self) -> String {
        match self.kind {
            Kind::OpenAi | Kind::Anthropic => self.endpoint.host_label(),
            Kind::ClaudeCli => "the claude command (it sends to Anthropic with your Claude login)".into(),
            Kind::CodexCli => "the codex command (it sends to OpenAI with your Codex login)".into(),
        }
    }

    /// A short name for the panel: "localhost:11434 · llama3.2 · local".
    pub fn label(&self) -> String {
        let place = match self.kind {
            Kind::OpenAi | Kind::Anthropic => self.endpoint.host_label(),
            k => k.program().to_owned(),
        };
        let model = if self.model.is_empty() { String::new() } else { format!(" · {}", self.model) };
        format!("{place}{model} · {}", if self.is_local() { "local" } else { "remote" })
    }
}

/// What's sent: instructions and the material.
#[derive(Clone, Debug, PartialEq)]
pub struct Prompt {
    pub system: String,
    pub user: String,
}

/// What a running request reports.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Text(String),
    /// Finished; with a note when the answer stopped early (a length limit,
    /// a refusal).
    Done(Option<String>),
    Failed(String),
}

/// Returned as the failure when the user cancelled.
pub const CANCELLED: &str = "Cancelled.";

/// A request on its way: events arrive on `rx`.
pub struct Job {
    pub rx: Receiver<Event>,
    cancel: Arc<AtomicBool>,
}

impl Job {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        // Dropping a job (a new request, the window closing) stops it.
        self.cancel();
    }
}

/// Send `prompt` to `provider` on a background thread. The key, if the
/// provider has one, is read from `keys` on that thread (the keychain can
/// take a moment, or ask the user).
pub fn start(provider: Provider, prompt: Prompt, keys: Arc<dyn keychain::KeyStore>, wake: Option<egui::Context>) -> Job {
    let (tx, rx) = channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let stop = cancel.clone();
    let spawned = std::thread::Builder::new().name("ai".into()).spawn(move || {
        let mut finished = false;
        let mut emit = |e: Event| {
            if finished {
                return;
            }
            finished = matches!(e, Event::Done(_) | Event::Failed(_));
            let _ = tx.send(e);
            if let Some(c) = &wake {
                c.request_repaint();
            }
        };
        run(&provider, &prompt, keys.as_ref(), &stop, &mut emit);
        // Ignored when the run already finished, as it should have.
        emit(Event::Failed("The request ended without an answer.".into()));
    });
    if let Err(e) = spawned {
        let (tx, rx) = channel();
        let _ = tx.send(Event::Failed(format!("Couldn't start the request: {e}")));
        return Job { rx, cancel };
    }
    Job { rx, cancel }
}

fn run(provider: &Provider, prompt: &Prompt, keys: &dyn keychain::KeyStore, cancel: &AtomicBool, emit: &mut dyn FnMut(Event)) {
    if provider.kind.is_cli() {
        return cli::stream(provider, prompt, cancel, emit);
    }
    let key = match provider.key_account() {
        Some(account) => match keys.get(&account) {
            Ok(k) => k,
            Err(e) => return emit(Event::Failed(format!("Couldn't read the API key: {e}"))),
        },
        None => None,
    };
    if provider.kind == Kind::Anthropic && key.is_none() {
        return emit(Event::Failed("Anthropic needs an API key: add one in Settings ▸ AI.".into()));
    }
    http::chat(provider, key.as_deref(), prompt, cancel, emit);
}

/// What Test Connection found.
pub type TestResult = Result<String, String>;

/// Check the provider answers (and lists its models, when it can) without
/// sending any of the user's text, on a background thread.
pub fn test_connection(provider: Provider, keys: Arc<dyn keychain::KeyStore>, wake: Option<egui::Context>) -> Receiver<TestResult> {
    let (tx, rx) = channel();
    let spawned = std::thread::Builder::new().name("ai-test".into()).spawn(move || {
        let result = if provider.kind.is_cli() {
            cli::version(&provider)
        } else {
            match provider.key_account().map(|a| keys.get(&a)).transpose() {
                Ok(key) => http::test(&provider, key.flatten().as_deref()),
                Err(e) => Err(format!("Couldn't read the API key: {e}")),
            }
        };
        let _ = tx.send(result);
        if let Some(c) = &wake {
            c.request_repaint();
        }
    });
    if let Err(e) = spawned {
        let (tx2, rx2) = channel();
        let _ = tx2.send(Err(format!("Couldn't start the test: {e}")));
        return rx2;
    }
    rx
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::session::Settings;

    pub fn settings(provider: &str, url: &str, model: &str) -> Settings {
        Settings { ai_enabled: true, ai_provider: provider.into(), ai_base_url: url.into(), ai_model: model.into(), ..Settings::default() }
    }

    #[test]
    fn local_and_remote_hosts() {
        for h in ["localhost", "LOCALHOST", "localhost.", "127.0.0.1", "127.8.9.10", "::1", "10.0.0.5", "192.168.1.20", "172.16.0.1", "172.31.255.255", "169.254.3.4", "fd12:3456::1", "fe80::1", "::ffff:192.168.0.2", "studio.local", "Mac-Mini.local"] {
            assert!(is_local_host(h), "{h} is local");
        }
        for h in ["api.openai.com", "openrouter.ai", "172.15.0.1", "172.32.0.1", "8.8.8.8", "100.64.0.1", "0.0.0.0", "2001:db8::1", "::ffff:8.8.8.8", "local", ".local", "localhost.example.com", "printer.local.example.com", "my-ollama.lan", "intranet", "192.168.1.20.nip.io"] {
            assert!(!is_local_host(h), "{h} is remote");
        }
    }

    #[test]
    fn endpoints_parse_strictly() {
        let e = Endpoint::parse("http://localhost:11434/v1/").unwrap();
        assert_eq!((e.https, e.host.as_str(), e.port, e.path.as_str()), (false, "localhost", Some(11434), "/v1"));
        assert_eq!(e.url("/chat/completions"), "http://localhost:11434/v1/chat/completions");
        assert_eq!(Endpoint::parse("HTTPS://OpenRouter.ai/api/v1").unwrap().url("/models"), "https://openrouter.ai/api/v1/models");
        let v6 = Endpoint::parse("http://[::1]:1234/v1").unwrap();
        assert_eq!((v6.host.as_str(), v6.origin()), ("::1", "http://[::1]:1234".to_owned()));
        assert!(v6.is_local());
        for bad in ["localhost:11434", "ftp://x", "file:///etc/passwd", "http://user:pw@host/v1", "http://host/v1?x=1", "http://host/v1#a", "http://ho st/", "http://host:0/", "http://host:99999/", "http://\"evil\"/", "http://host/\nheader", "http://[zz]/", "http:///v1"] {
            assert!(Endpoint::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn providers_from_settings() {
        let p = Provider::from_settings(&settings("openai", "http://localhost:11434/v1", "llama3.2")).unwrap();
        assert!(p.is_local());
        assert_eq!(p.key_account().unwrap(), "openai http://localhost:11434");
        assert!(p.label().ends_with("local"));
        // A remote endpoint needs https; a missing model is explained.
        assert!(Provider::from_settings(&settings("openai", "http://api.example.com/v1", "m")).unwrap_err().contains("https"));
        assert!(Provider::from_settings(&settings("openai", "https://api.example.com/v1", "")).unwrap_err().contains("model"));
        let r = Provider::from_settings(&settings("openai", "https://openrouter.ai/api/v1", "x/y:free")).unwrap();
        assert!(!r.is_local());
        assert_eq!(r.consent_id(), "openai https://openrouter.ai");
        // Models with shell or JSON characters are refused.
        assert!(Provider::from_settings(&settings("openai", "http://localhost/v1", "a;rm -rf")).is_err());
        let a = Provider::from_settings(&settings("anthropic", "", "")).unwrap();
        assert_eq!((a.is_local(), a.model.as_str()), (false, "claude-sonnet-5-5"));
        assert_eq!(a.key_account().unwrap(), "anthropic https://api.anthropic.com");
        for cli in ["claude_cli", "codex_cli"] {
            let c = Provider::from_settings(&settings(cli, "http://localhost/v1", "")).unwrap();
            assert!(!c.is_local(), "command-line tools send to their vendor");
            assert_eq!(c.key_account(), None);
        }
        assert!(Provider::from_settings(&settings("gpt", "", "")).is_err());
    }
}
