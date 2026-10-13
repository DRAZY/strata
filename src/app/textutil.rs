//! Text utilities (Tools ▸ Text): JSON formatting, encodings for the web,
//! inserted values. Each applies to every non-empty selection, or the whole
//! file when nothing is selected, as one undo step.

use super::StrataApp;

pub fn json_format(s: &str) -> Result<String, String> {
    let v: serde_json::Value = serde_json::from_str(s).map_err(|e| format!("Not valid JSON: {e}"))?;
    serde_json::to_string_pretty(&v).map_err(|e| e.to_string())
}

pub fn json_minify(s: &str) -> Result<String, String> {
    let v: serde_json::Value = serde_json::from_str(s).map_err(|e| format!("Not valid JSON: {e}"))?;
    serde_json::to_string(&v).map_err(|e| e.to_string())
}

pub fn json_sort_keys(s: &str) -> Result<String, String> {
    fn sort(v: serde_json::Value) -> serde_json::Value {
        match v {
            serde_json::Value::Object(map) => {
                let mut entries: Vec<_> = map.into_iter().collect();
                entries.sort_by(|a, b| a.0.cmp(&b.0));
                serde_json::Value::Object(entries.into_iter().map(|(k, v)| (k, sort(v))).collect())
            }
            serde_json::Value::Array(a) => serde_json::Value::Array(a.into_iter().map(sort).collect()),
            other => other,
        }
    }
    let v: serde_json::Value = serde_json::from_str(s).map_err(|e| format!("Not valid JSON: {e}"))?;
    serde_json::to_string_pretty(&sort(v)).map_err(|e| e.to_string())
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(s: &str) -> Result<String, String> {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |acc, (i, &b)| acc | (b as u32) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(B64[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    Ok(out)
}

pub fn base64_decode(s: &str) -> Result<String, String> {
    let mut bits = 0u32;
    let mut nbits = 0;
    let mut out = Vec::new();
    for c in s.chars().filter(|c| !c.is_whitespace() && *c != '=') {
        let v = match c {
            'A'..='Z' => c as u32 - 'A' as u32,
            'a'..='z' => c as u32 - 'a' as u32 + 26,
            '0'..='9' => c as u32 - '0' as u32 + 52,
            '+' | '-' => 62,
            '/' | '_' => 63,
            _ => return Err(format!("Not Base64: unexpected '{c}'")),
        };
        bits = bits << 6 | v;
        nbits += 6;
        if nbits >= 8 {
            nbits -= 8;
            out.push((bits >> nbits) as u8);
            bits &= (1 << nbits) - 1;
        }
    }
    String::from_utf8(out).map_err(|_| "Decoded bytes aren't text (UTF-8)".into())
}

pub fn url_encode(s: &str) -> Result<String, String> {
    Ok(s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect())
}

pub fn url_decode(s: &str) -> Result<String, String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                let hex = std::str::from_utf8(&b[i + 1..i + 3]).map_err(|_| "Bad % escape")?;
                out.push(u8::from_str_radix(hex, 16).map_err(|_| format!("Bad % escape: %{hex}"))?);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|_| "Decoded bytes aren't text (UTF-8)".into())
}

pub fn html_escape(s: &str) -> Result<String, String> {
    Ok(s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;"))
}

pub fn html_unescape(s: &str) -> Result<String, String> {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';').filter(|&e| e <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let c = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some('\u{a0}'),
            e if e.starts_with("#x") || e.starts_with("#X") => u32::from_str_radix(&e[2..], 16).ok().and_then(char::from_u32),
            e if e.starts_with('#') => e[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match c {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    Ok(out)
}

/// A random (version 4) UUID, from the system clock and a mixer; fine for ids, not crypto.
pub fn uuid_v4() -> String {
    let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()) as u64 ^ std::process::id() as u64;
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let (a, b) = (next(), next());
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&a.to_be_bytes());
    bytes[8..].copy_from_slice(&b.to_be_bytes());
    bytes[6] = bytes[6] & 0x0f | 0x40;
    bytes[8] = bytes[8] & 0x3f | 0x80;
    let h: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

/// Local date and time as "YYYY-MM-DD HH:MM", from the system's `date` where available.
pub fn now_string() -> String {
    let cmd = if cfg!(windows) {
        std::process::Command::new("powershell").args(["-NoProfile", "-Command", "Get-Date -Format 'yyyy-MM-dd HH:mm'"]).output()
    } else {
        std::process::Command::new("date").arg("+%Y-%m-%d %H:%M").output()
    };
    cmd.ok().and_then(|o| String::from_utf8(o.stdout).ok()).map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).unwrap_or_default()
}

impl StrataApp {
    /// Replace each non-empty selection (or the whole file) with `f(text)`.
    pub(super) fn transform(&mut self, f: fn(&str) -> Result<String, String>) {
        let e = &mut self.docs[self.active].editor;
        let mut ranges: Vec<std::ops::Range<usize>> = e.selections().iter().filter(|s| !s.is_empty()).map(|s| s.range()).collect();
        if ranges.is_empty() {
            ranges.push(0..e.buffer.len_chars());
        }
        let mut edits = Vec::with_capacity(ranges.len());
        for r in ranges {
            match f(&e.buffer.slice(r.clone())) {
                Ok(t) => edits.push((r, t)),
                Err(msg) => {
                    self.status = msg;
                    return;
                }
            }
        }
        e.replace_ranges(edits);
        self.docs[self.active].view.reveal();
    }

    pub(super) fn insert_at_cursors(&mut self, text: &str) {
        if !text.is_empty() {
            self.docs[self.active].editor.insert_text(text);
        }
    }

    /// Sublime's save-time options: trim trailing whitespace, end with a newline.
    pub(super) fn before_save(&mut self, i: usize) {
        let (trim, newline) = (self.settings.trim_trailing_white_space_on_save, self.settings.ensure_newline_at_eof_on_save);
        let e = &mut self.docs[i].editor;
        if trim {
            let sels: Vec<std::ops::Range<usize>> = e.selections().iter().map(|s| s.range()).collect();
            e.select_ranges(&[0..0], 0);
            e.trim_trailing_whitespace();
            let len = e.buffer.len_chars();
            let sels: Vec<_> = sels.into_iter().map(|r| r.start.min(len)..r.end.min(len)).collect();
            e.select_ranges(&sels, 0);
        }
        let len = e.buffer.len_chars();
        if newline && len > 0 && e.buffer.char(len - 1) != '\n' {
            let ending = e.buffer.line_ending().as_str().to_owned();
            let sels: Vec<std::ops::Range<usize>> = e.selections().iter().map(|s| s.range()).collect();
            e.replace_ranges(vec![(len..len, ending)]);
            e.select_ranges(&sels, 0);
        }
    }

    /// "N words" for the selection (or the file, when small enough to count every frame).
    pub(super) fn word_count(&self) -> Option<String> {
        let e = &self.docs[self.active].editor;
        let selected: Vec<String> = e.selections().iter().filter(|s| !s.is_empty()).map(|s| e.buffer.slice(s.range())).collect();
        let (text, what) = if !selected.is_empty() {
            (selected.join(" "), " selected")
        } else if e.buffer.len_chars() < 200_000 {
            (e.buffer.slice(0..e.buffer.len_chars()), "")
        } else {
            return None;
        };
        let n = text.split_whitespace().count();
        Some(format!("{n} word{}{what}", if n == 1 { "" } else { "s" }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_round_trips_and_errors() {
        assert_eq!(json_minify("{ \"b\": 1, \"a\": [1, 2] }").unwrap(), r#"{"b":1,"a":[1,2]}"#);
        assert_eq!(json_sort_keys(r#"{"b":1,"a":{"d":1,"c":2}}"#).unwrap().replace([' ', '\n'], ""), r#"{"a":{"c":2,"d":1},"b":1}"#);
        assert!(json_format("{nope").is_err());
    }

    #[test]
    fn base64_matches_known_values() {
        for (plain, enc) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("héllo", "aMOpbGxv")] {
            assert_eq!(base64_encode(plain).unwrap(), enc);
            assert_eq!(base64_decode(enc).unwrap(), plain);
        }
        assert!(base64_decode("@@").is_err());
    }

    #[test]
    fn url_and_html_codecs() {
        assert_eq!(url_encode("a b&c/é").unwrap(), "a%20b%26c%2F%C3%A9");
        assert_eq!(url_decode("a%20b+c%2F%C3%A9").unwrap(), "a b c/é");
        assert_eq!(html_escape("<a href=\"x\">&</a>").unwrap(), "&lt;a href=&quot;x&quot;&gt;&amp;&lt;/a&gt;");
        assert_eq!(html_unescape("&lt;p&gt; &amp; &#65;&#x42; &bogus; &").unwrap(), "<p> & AB &bogus; &");
    }

    #[test]
    fn uuid_shape() {
        let u = uuid_v4();
        assert_eq!(u.len(), 36);
        assert_eq!(&u[14..15], "4");
    }
}
