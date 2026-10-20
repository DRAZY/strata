//! Server-sent events, as OpenAI-compatible servers and the Anthropic API
//! stream their answers: `event:` and `data:` lines, a blank line ending
//! each event.

#[derive(Default)]
pub struct Parser {
    event: String,
    data: Vec<String>,
}

/// One event: its name ("" when the server gave none) and its data lines joined.
#[derive(Debug, PartialEq)]
pub struct Message {
    pub event: String,
    pub data: String,
}

impl Parser {
    /// Feed one line (without its line break); a finished event comes back.
    pub fn line(&mut self, line: &str) -> Option<Message> {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            return self.flush();
        }
        if line.starts_with(':') {
            return None; // a comment (keep-alive)
        }
        let (field, value) = match line.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (line, ""),
        };
        match field {
            "event" => self.event = value.to_owned(),
            "data" => self.data.push(value.to_owned()),
            _ => {} // id, retry: nothing to do with them
        }
        None
    }

    /// The event in progress, if any (at the end of the stream).
    pub fn flush(&mut self) -> Option<Message> {
        if self.data.is_empty() {
            self.event.clear();
            return None;
        }
        let m = Message { event: std::mem::take(&mut self.event), data: self.data.join("\n") };
        self.data.clear();
        Some(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_and_comments() {
        let mut p = Parser::default();
        let mut out = Vec::new();
        for l in [": keep-alive", "event: content_block_delta", "data: {\"a\":1}", "", "data:{\"b\":2}\r", "\r", "data: one", "data: two", "id: 7", "", "", "data: [DONE]"] {
            out.extend(p.line(l));
        }
        out.extend(p.flush());
        assert_eq!(
            out,
            vec![
                Message { event: "content_block_delta".into(), data: "{\"a\":1}".into() },
                Message { event: String::new(), data: "{\"b\":2}".into() },
                Message { event: String::new(), data: "one\ntwo".into() },
                Message { event: String::new(), data: "[DONE]".into() },
            ]
        );
    }
}
