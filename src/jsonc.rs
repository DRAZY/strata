//! Editing JSON-with-comments files in place: change one top-level key's
//! value (or add the key) and leave every other byte alone, so the comments,
//! order and formatting someone wrote in settings.json survive Strata
//! saving a zoom level or a View toggle.

use serde_json::Value;

/// Index of the first byte at or after `i` that isn't whitespace or part of a comment.
fn skip(b: &[u8], mut i: usize) -> usize {
    loop {
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if b.get(i) == Some(&b'/') && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if b.get(i) == Some(&b'/') && b.get(i + 1) == Some(&b'*') {
            i = find(b, i + 2, b"*/").map_or(b.len(), |e| e + 2);
        } else {
            return i;
        }
    }
}

fn find(b: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    (from..b.len().saturating_sub(needle.len() - 1)).find(|&k| b[k..].starts_with(needle))
}

/// End (exclusive) of the string starting at `i` (a `"`).
fn string_end(b: &[u8], mut i: usize) -> Option<usize> {
    i += 1;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            b'"' => return Some(i + 1),
            _ => i += 1,
        }
    }
    None
}

/// End (exclusive) of the value starting at `i`.
fn value_end(b: &[u8], i: usize) -> Option<usize> {
    match *b.get(i)? {
        b'"' => string_end(b, i),
        b'{' | b'[' => {
            let mut depth = 0usize;
            let mut k = i;
            while k < b.len() {
                k = skip(b, k);
                match *b.get(k)? {
                    b'"' => {
                        k = string_end(b, k)?;
                        continue;
                    }
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(k + 1);
                        }
                    }
                    _ => {}
                }
                k += 1;
            }
            None
        }
        _ => {
            let mut k = i;
            while k < b.len() && !matches!(b[k], b',' | b'}' | b']' | b'/') && !b[k].is_ascii_whitespace() {
                k += 1;
            }
            (k > i).then_some(k)
        }
    }
}

/// One `"key": value` of the top-level object.
struct Member {
    key: String,
    key_start: usize,
    value: std::ops::Range<usize>,
    /// Position of the comma after the value, if any.
    comma: Option<usize>,
}

/// The top-level object's members and the position of its closing `}`.
fn members(text: &str) -> Option<(usize, Vec<Member>, usize)> {
    let b = text.as_bytes();
    let open = skip(b, 0);
    if b.get(open) != Some(&b'{') {
        return None;
    }
    let mut out = Vec::new();
    let mut i = open + 1;
    loop {
        i = skip(b, i);
        match *b.get(i)? {
            b'}' => return Some((open, out, i)),
            b'"' => {
                let key_end = string_end(b, i)?;
                let key: String = serde_json::from_str(&text[i..key_end]).ok()?;
                let colon = skip(b, key_end);
                if b.get(colon) != Some(&b':') {
                    return None;
                }
                let start = skip(b, colon + 1);
                let end = value_end(b, start)?;
                let after = skip(b, end);
                let comma = (b.get(after) == Some(&b',')).then_some(after);
                out.push(Member { key, key_start: i, value: start..end, comma });
                i = comma.map_or(after, |c| c + 1);
                if comma.is_none() && b.get(after) != Some(&b'}') {
                    return None;
                }
            }
            _ => return None,
        }
    }
}

/// Leading whitespace of the line holding byte `at`.
fn indent_at(text: &str, at: usize) -> &str {
    let line = text[..at].rfind('\n').map_or(0, |n| n + 1);
    let rest = &text[line..];
    &rest[..rest.len() - rest.trim_start_matches([' ', '\t']).len()]
}

/// `value` as pretty JSON for a member indented by `indent`: nested levels
/// step by the same amount, as the file's own members do.
fn render(value: &Value, indent: &str) -> String {
    use serde::Serialize;
    let step = if indent.is_empty() { "    " } else { indent };
    let mut out = Vec::new();
    let mut ser = serde_json::Serializer::with_formatter(&mut out, serde_json::ser::PrettyFormatter::with_indent(step.as_bytes()));
    if value.serialize(&mut ser).is_err() {
        return "null".into();
    }
    String::from_utf8(out).unwrap_or_default().replace('\n', &format!("\n{indent}"))
}

/// `text` (a JSON object, comments and trailing commas allowed) with the
/// top-level `key` set to `value`: only that value's text changes, or the key
/// is added after the last one. None if `text` isn't an object.
pub fn set_key(text: &str, key: &str, value: &Value) -> Option<String> {
    let (open, members, close) = members(text)?;
    // serde_json keeps the last of duplicate keys; so do we.
    if let Some(m) = members.iter().rev().find(|m| m.key == key) {
        let indent = indent_at(text, m.key_start);
        return Some(format!("{}{}{}", &text[..m.value.start], render(value, indent), &text[m.value.end..]));
    }
    let name = serde_json::to_string(key).ok()?;
    let Some(last) = members.last() else {
        let indent = "    ";
        let inner = &text[open + 1..close];
        let tail = if inner.trim().is_empty() { "\n".to_owned() } else { inner.to_owned() };
        return Some(format!("{}\n{indent}{name}: {}{tail}{}", &text[..=open], render(value, indent), &text[close..]));
    };
    let indent = indent_at(text, members[0].key_start).to_owned();
    let new = format!("{name}: {}", render(value, &indent));
    // After the line the last member ends on (keeping a comment there with it).
    let after = last.comma.map_or(last.value.end, |c| c + 1);
    let line_end = text[after..close].find('\n').map_or(close, |n| after + n);
    let on_own_line = line_end < close;
    let new = if on_own_line { format!("\n{indent}{new}") } else { format!("\n{indent}{new}\n") };
    Some(match last.comma {
        // A trailing comma: the new key gets one too.
        Some(_) => format!("{}{new},{}", &text[..line_end], &text[line_end..]),
        None => format!("{},{}{new}{}", &text[..last.value.end], &text[last.value.end..line_end], &text[line_end..]),
    })
}

/// Equal as settings: numbers by value (14 is 14.0).
pub fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same(p, q)),
        (Value::Object(x), Value::Object(y)) => x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w))),
        _ => a == b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(text: &str) -> Value {
        serde_json::from_str(&crate::session::strip_jsonc(text)).unwrap()
    }

    #[test]
    fn changes_only_the_value() {
        let text = "// My settings\n{\n    // big text\n    \"font_size\": 14, // was 12\n    \"theme\": \"Nord\",\n    /* keep */ \"rulers\": [80, 100],\n}\n";
        let out = set_key(text, "font_size", &json!(16.0)).unwrap();
        assert_eq!(out, text.replace("\"font_size\": 14,", "\"font_size\": 16.0,"));
        let out = set_key(&out, "rulers", &json!([72])).unwrap();
        assert!(out.contains("/* keep */ \"rulers\": [\n        72\n    ],\n}"), "{out}");
        assert_eq!(parse(&out)["theme"], "Nord");
    }

    #[test]
    fn adds_a_missing_key_after_the_last() {
        // Trailing comma kept, comment on the last line stays with its key.
        let text = "{\n  \"a\": 1,\n  \"b\": \"x\", // note\n}\n";
        let out = set_key(text, "minimap", &json!(false)).unwrap();
        assert_eq!(out, "{\n  \"a\": 1,\n  \"b\": \"x\", // note\n  \"minimap\": false,\n}\n");
        // No trailing comma: one is added after the old last value.
        let text = "{\n    \"a\": 1 // one\n}";
        let out = set_key(text, "b", &json!("two")).unwrap();
        assert_eq!(out, "{\n    \"a\": 1, // one\n    \"b\": \"two\"\n}");
        assert_eq!(parse(&out), json!({"a": 1, "b": "two"}));
        // Everything on one line.
        let out = set_key("{\"a\": 1}", "b", &json!(2)).unwrap();
        assert_eq!(parse(&out), json!({"a": 1, "b": 2}));
        // An empty object, with and without a comment inside.
        assert_eq!(parse(&set_key("{}", "b", &json!(2)).unwrap()), json!({"b": 2}));
        let out = set_key("{\n  // nothing yet\n}", "b", &json!(2)).unwrap();
        assert!(out.contains("// nothing yet"));
        assert_eq!(parse(&out), json!({"b": 2}));
    }

    #[test]
    fn strings_and_comments_dont_confuse_it() {
        let text = "{\n  \"a\": \"} // not a comment, \\\"quoted\\\" {\",\n  /* \"font_size\": 1, */\n  \"nested\": {\"font_size\": [1, {\"x\": \"]\"}]},\n  \"font_size\": 12\n}";
        let out = set_key(text, "font_size", &json!(20)).unwrap();
        let v = parse(&out);
        assert_eq!(v["font_size"], 20);
        assert_eq!(v["nested"]["font_size"][0], 1, "the nested key is left alone");
        assert!(out.contains("/* \"font_size\": 1, */"));
        assert!(set_key("[1, 2]", "a", &json!(1)).is_none(), "not an object");
        assert!(set_key("{\"a\": }", "a", &json!(1)).is_none(), "broken");
    }

    #[test]
    fn numbers_compare_by_value() {
        assert!(same(&json!(14), &json!(14.0)));
        assert!(same(&json!({"a": [1]}), &json!({"a": [1.0]})));
        assert!(!same(&json!(14), &json!(15)));
    }
}
