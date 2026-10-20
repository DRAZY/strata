//! The language server base protocol: JSON messages, each after a header
//! block that gives its length (`Content-Length: 42\r\n\r\n{...}`).

use serde_json::Value;

/// A message bigger than this is refused rather than buffered (a confused
/// server shouldn't be able to make Strata allocate gigabytes).
pub const MAX_MESSAGE: usize = 256 << 20;
/// Header blocks are a few dozen bytes; this much without one ending means
/// the stream isn't headers at all.
const MAX_HEADER: usize = 16 << 10;

/// A message framed for the wire.
pub fn encode(msg: &Value) -> Vec<u8> {
    let body = msg.to_string();
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(body.as_bytes());
    out
}

#[derive(Debug, PartialEq)]
pub enum FrameError {
    /// A header block without a usable `Content-Length` (it's skipped).
    BadHeader(String),
    /// The message says it's bigger than `MAX_MESSAGE`.
    TooLarge(usize),
    /// The body isn't JSON (the stream stays in step: its length was known).
    BadJson(String),
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            FrameError::BadHeader(h) => write!(f, "bad message header: {h}"),
            FrameError::TooLarge(n) => write!(f, "message of {n} bytes is too large"),
            FrameError::BadJson(e) => write!(f, "message isn't JSON: {e}"),
        }
    }
}

/// Splits a byte stream into messages, whatever sizes the reads come in.
#[derive(Default)]
pub struct Decoder {
    buf: Vec<u8>,
    /// Bytes before this were already handed out.
    start: usize,
    /// Body length from a header already read, waiting for the body.
    body: Option<usize>,
    /// Skipping the body of a message that was too large.
    skip: usize,
    /// A smaller cap than `MAX_MESSAGE` (plugins get one).
    limit: Option<usize>,
}

impl Decoder {
    pub fn with_limit(limit: usize) -> Self {
        Self { limit: Some(limit.min(MAX_MESSAGE)), ..Self::default() }
    }

    pub fn push(&mut self, bytes: &[u8]) {
        let mut bytes = bytes;
        if self.skip > 0 {
            let n = self.skip.min(bytes.len());
            self.skip -= n;
            bytes = &bytes[n..];
        }
        if self.start > 0 && self.start * 2 >= self.buf.len() {
            self.buf.drain(..self.start);
            self.start = 0;
        }
        self.buf.extend_from_slice(bytes);
    }

    /// The next complete message, if one has arrived.
    pub fn next(&mut self) -> Option<Result<Value, FrameError>> {
        loop {
            if self.skip > 0 {
                return None;
            }
            let len = match self.body {
                Some(len) => len,
                None => match self.header()? {
                    Ok(len) => len,
                    Err(e) => return Some(Err(e)),
                },
            };
            let avail = self.buf.len() - self.start;
            if len > self.limit.unwrap_or(MAX_MESSAGE) {
                self.body = None;
                self.skip = len.saturating_sub(avail);
                self.start += len.min(avail);
                return Some(Err(FrameError::TooLarge(len)));
            }
            if avail < len {
                self.body = Some(len);
                return None;
            }
            self.body = None;
            let body = &self.buf[self.start..self.start + len];
            self.start += len;
            let parsed = serde_json::from_slice::<Value>(body).map_err(|e| FrameError::BadJson(e.to_string()));
            if len == 0 {
                // An empty body carries nothing (and isn't JSON); look further.
                continue;
            }
            return Some(parsed);
        }
    }

    /// Read a header block: the body's length, an error for a block without
    /// one, or None until the block is complete.
    fn header(&mut self) -> Option<Result<usize, FrameError>> {
        let rest = &self.buf[self.start..];
        let Some(end) = find(rest, b"\r\n\r\n") else {
            if rest.len() > MAX_HEADER {
                // Not a header: drop it and look for the next one.
                let keep = rest.len().saturating_sub(32);
                let junk = String::from_utf8_lossy(&rest[..keep.min(80)]).into_owned();
                self.start += keep;
                return Some(Err(FrameError::BadHeader(junk)));
            }
            return None;
        };
        let block = String::from_utf8_lossy(&rest[..end]).into_owned();
        self.start += end + 4;
        // Lines may be preceded by stray output (some servers print to
        // stdout before speaking the protocol); only Content-Length matters.
        let mut length = None;
        for line in block.split(['\r', '\n']) {
            // (Lowercasing ASCII keeps byte offsets.)
            if let Some(at) = line.to_ascii_lowercase().find("content-length")
                && let Some(value) = line[at + "content-length".len()..].trim_start().strip_prefix(':')
            {
                length = value.trim().parse::<usize>().ok();
            }
        }
        Some(length.ok_or_else(|| FrameError::BadHeader(block.chars().take(80).collect())))
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn all(d: &mut Decoder) -> Vec<Result<Value, FrameError>> {
        std::iter::from_fn(|| d.next()).collect()
    }

    #[test]
    fn round_trips_and_handles_reads_of_any_size() {
        let a = json!({"jsonrpc": "2.0", "id": 1, "result": {"text": "héllo 👋"}});
        let b = json!({"jsonrpc": "2.0", "method": "x", "params": [1, 2, 3]});
        let mut wire = encode(&a);
        wire.extend(encode(&b));
        // One byte at a time: nothing until a message is whole.
        let mut d = Decoder::default();
        let mut got = Vec::new();
        for byte in &wire {
            d.push(std::slice::from_ref(byte));
            got.extend(all(&mut d));
        }
        assert_eq!(got, vec![Ok(a.clone()), Ok(b.clone())]);
        // Both in one read.
        let mut d = Decoder::default();
        d.push(&wire);
        assert_eq!(all(&mut d), vec![Ok(a.clone()), Ok(b.clone())]);
        // Split inside the header and inside the multi-byte characters.
        for cut in [3, 17, 20, 40, 45, wire.len() - 1] {
            let mut d = Decoder::default();
            d.push(&wire[..cut]);
            let mut got = all(&mut d);
            d.push(&wire[cut..]);
            got.extend(all(&mut d));
            assert_eq!(got, vec![Ok(a.clone()), Ok(b.clone())], "cut at {cut}");
        }
    }

    #[test]
    fn content_length_counts_bytes_not_chars() {
        let msg = json!({"s": "日本語"});
        let wire = encode(&msg);
        let header = String::from_utf8_lossy(&wire[..wire.iter().position(|&b| b == b'\r').unwrap()]).into_owned();
        assert_eq!(header, format!("Content-Length: {}", msg.to_string().len()));
        assert!(msg.to_string().len() > msg.to_string().chars().count());
    }

    #[test]
    fn header_edge_cases() {
        let body = br#"{"a":1}"#;
        // Other headers, any case, any order, spaces around the value.
        let mut d = Decoder::default();
        d.push(b"Content-Type: application/vscode-jsonrpc; charset=utf-8\r\ncontent-LENGTH:   7  \r\n\r\n");
        d.push(body);
        assert_eq!(all(&mut d), vec![Ok(json!({"a": 1}))]);
        // No length: an error, and the next message still comes through.
        let mut d = Decoder::default();
        d.push(b"Content-Type: x\r\n\r\nContent-Length: 7\r\n\r\n");
        d.push(body);
        let got = all(&mut d);
        assert!(matches!(got[0], Err(FrameError::BadHeader(_))));
        assert_eq!(got[1], Ok(json!({"a": 1})));
        // A length that isn't a number.
        let mut d = Decoder::default();
        d.push(b"Content-Length: seven\r\n\r\n");
        assert!(matches!(d.next(), Some(Err(FrameError::BadHeader(_)))));
        // Stray output before the first header (a test harness, a chatty server).
        let mut d = Decoder::default();
        d.push(b"\nrunning 1 test\nContent-Length: 7\r\n\r\n");
        d.push(body);
        assert_eq!(all(&mut d), vec![Ok(json!({"a": 1}))]);
        // A zero length is skipped.
        let mut d = Decoder::default();
        d.push(b"Content-Length: 0\r\n\r\nContent-Length: 7\r\n\r\n{\"a\":1}");
        assert_eq!(all(&mut d), vec![Ok(json!({"a": 1}))]);
    }

    #[test]
    fn bad_json_and_huge_messages_keep_the_stream_in_step() {
        let mut d = Decoder::default();
        d.push(b"Content-Length: 5\r\n\r\n{nope");
        d.push(&encode(&json!(2)));
        let got = all(&mut d);
        assert!(matches!(got[0], Err(FrameError::BadJson(_))));
        assert_eq!(got[1], Ok(json!(2)));

        // Too large: refused, its body skipped as it arrives, then back in step.
        let mut d = Decoder::default();
        let n = MAX_MESSAGE + 10;
        d.push(format!("Content-Length: {n}\r\n\r\nabc").as_bytes());
        assert_eq!(d.next(), Some(Err(FrameError::TooLarge(n))));
        let mut left = n - 3;
        let chunk = vec![b'x'; 1 << 20];
        while left > 0 {
            let k = left.min(chunk.len());
            d.push(&chunk[..k]);
            assert_eq!(d.next(), None);
            left -= k;
        }
        d.push(&encode(&json!("after")));
        assert_eq!(all(&mut d), vec![Ok(json!("after"))]);

        // Garbage that never ends a header block doesn't grow without bound.
        let mut d = Decoder::default();
        d.push(&vec![b'z'; MAX_HEADER + 100]);
        assert!(matches!(d.next(), Some(Err(FrameError::BadHeader(_)))));
        d.push(&encode(&json!(3)));
        assert_eq!(all(&mut d), vec![Ok(json!(3))]);
    }
}
