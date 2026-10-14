//! Raw HTML from a Markdown file, made safe for the pages Strata writes
//! (Export as HTML, Copy as HTML, printing). A Markdown file can come from
//! anywhere, and an exported page opened in a browser runs whatever script it
//! carries, so only plain formatting tags pass, with only harmless attributes;
//! links and images keep only http, https and mailto (or relative) targets.
//! Everything else (`<script>`, `<iframe>`, `<style>`, `on*=` handlers,
//! `javascript:` URLs, comments) is shown as text or dropped.

use regex::Regex;
use std::sync::LazyLock;

/// Tags that pass (with allowed attributes only).
const TAGS: &[&str] = &[
    "a", "abbr", "b", "bdi", "bdo", "blockquote", "br", "caption", "center", "cite", "code", "col", "colgroup", "dd", "del", "details", "dfn", "div", "dl", "dt", "em", "figcaption", "figure", "h1", "h2", "h3", "h4", "h5", "h6", "hr", "i", "img", "ins", "kbd", "li", "mark", "ol", "p", "picture", "pre", "q", "rp", "rt", "ruby", "s", "samp", "small", "span", "strike", "strong", "sub", "summary", "sup", "table", "tbody", "td", "tfoot", "th", "thead", "time", "tr", "tt", "u", "ul", "var", "wbr",
];
/// Attributes that pass. `href` and `src` also need a safe URL.
const ATTRS: &[&str] = &["href", "src", "alt", "title", "width", "height", "align", "valign", "colspan", "rowspan", "open", "id", "name", "lang", "dir", "start", "reversed", "type", "datetime", "cite", "class"];
const URL_ATTRS: &[&str] = &["href", "src", "cite"];

static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^<>]*>").unwrap());
static TAG_PARTS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)^<(/?)([A-Za-z][A-Za-z0-9-]*)(.*?)(/?)>$").unwrap());
static ATTR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"([^\s"'<>/=]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'=<>`]+)))?"#).unwrap());

fn escape_text(s: &str) -> String {
    s.replace('<', "&lt;").replace('>', "&gt;")
}

fn escape_attr(s: &str) -> String {
    s.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Decode the character references a browser would in an attribute value.
fn decode_entities(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let end = rest[1..].find(|c: char| !(c.is_ascii_alphanumeric() || c == '#')).map_or(rest.len(), |e| e + 1);
        let name = &rest[1..end];
        let decoded = if let Some(num) = name.strip_prefix('#') {
            let n = match num.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok(),
                None => num.parse().ok(),
            };
            n.and_then(char::from_u32).map(String::from)
        } else {
            match name.to_ascii_lowercase().as_str() {
                "amp" => Some("&".into()),
                "lt" => Some("<".into()),
                "gt" => Some(">".into()),
                "quot" => Some("\"".into()),
                "apos" => Some("'".into()),
                "colon" => Some(":".into()),
                "tab" => Some("\t".into()),
                "newline" => Some("\n".into()),
                _ => None,
            }
        };
        match decoded {
            Some(d) if end > 1 => {
                out.push_str(&d);
                rest = rest[end..].strip_prefix(';').unwrap_or(&rest[end..]);
            }
            _ => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// A link or image target that's safe to keep: relative, or http(s)/mailto.
/// Browsers ignore whitespace and control characters inside a scheme, so
/// those don't count when finding it.
pub fn safe_url(url: &str) -> bool {
    let squeezed: String = url.chars().filter(|c| !c.is_whitespace() && !c.is_control()).collect::<String>().to_ascii_lowercase();
    match squeezed.find([':', '/', '?', '#']) {
        Some(i) if squeezed[i..].starts_with(':') => matches!(&squeezed[..i], "http" | "https" | "mailto"),
        _ => true,
    }
}

/// One tag, rebuilt from its allowed parts, or `None` to show it as text.
fn clean_tag(tag: &str) -> Option<String> {
    let caps = TAG_PARTS.captures(tag)?;
    let closing = !caps[1].is_empty();
    let name = caps[2].to_ascii_lowercase();
    if !TAGS.contains(&name.as_str()) {
        return None;
    }
    if closing {
        return Some(format!("</{name}>"));
    }
    let mut out = format!("<{name}");
    for a in ATTR.captures_iter(&caps[3]) {
        let attr = a[1].to_ascii_lowercase();
        if !ATTRS.contains(&attr.as_str()) {
            continue;
        }
        match a.get(2).or(a.get(3)).or(a.get(4)) {
            Some(v) => {
                let value = decode_entities(v.as_str());
                if URL_ATTRS.contains(&attr.as_str()) && !safe_url(&value) {
                    continue;
                }
                out.push_str(&format!(" {attr}=\"{}\"", escape_attr(&value)));
            }
            None if !URL_ATTRS.contains(&attr.as_str()) => out.push_str(&format!(" {attr}")),
            None => {}
        }
    }
    if !caps[4].is_empty() {
        out.push_str(" /");
    }
    out.push('>');
    Some(out)
}

/// Raw HTML (a Markdown HTML block or inline tag), made safe.
pub fn sanitize(raw: &str) -> String {
    let mut out = String::new();
    let mut last = 0;
    for m in TAG.find_iter(raw) {
        out.push_str(&escape_text(&raw[last..m.start()]));
        let tag = m.as_str();
        // Comments, doctypes, processing instructions: dropped.
        if !(tag.starts_with("<!") || tag.starts_with("<?")) {
            out.push_str(&clean_tag(tag).unwrap_or_else(|| escape_text(tag)));
        }
        last = m.end();
    }
    out.push_str(&escape_text(&raw[last..]));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting_tags_pass() {
        assert_eq!(sanitize("<b>bold</b> and <br/>"), "<b>bold</b> and <br />");
        assert_eq!(sanitize("<details open><summary>More</summary>"), "<details open><summary>More</summary>");
        assert_eq!(sanitize(r#"<a href="https://example.com" title='t'>x</a>"#), r#"<a href="https://example.com" title="t">x</a>"#);
        assert_eq!(sanitize(r#"<img src="pic.png" alt="a &amp; b" width=40>"#), r#"<img src="pic.png" alt="a &amp; b" width="40">"#);
        assert_eq!(sanitize("a &amp; b"), "a &amp; b", "entities in text stay");
    }

    #[test]
    fn scripts_handlers_and_frames_do_not() {
        for evil in [
            "<script>alert(1)</script>",
            "<SCRIPT SRC=//evil.example/x.js></SCRIPT>",
            "<iframe src=\"https://evil.example\"></iframe>",
            "<style>body{background:url(https://evil.example)}</style>",
            "<object data=x></object>",
            "<svg onload=alert(1)>",
            "<form action=https://evil.example><input name=p></form>",
            "<meta http-equiv=refresh content=\"0;url=https://evil.example\">",
        ] {
            let out = sanitize(evil);
            assert!(!out.contains('<'), "{evil} -> {out}");
        }
        assert_eq!(sanitize("<img src=x onerror=alert(1)>"), "<img src=\"x\">");
        assert_eq!(sanitize("<p onclick=\"steal()\" style=\"x\">hi</p>"), "<p>hi</p>");
        assert_eq!(sanitize("<!-- hidden --><b>x</b>"), "<b>x</b>");
    }

    #[test]
    fn script_urls_are_dropped_however_spelled() {
        for href in [
            "javascript:alert(1)",
            "JaVaScRiPt:alert(1)",
            " javascript:alert(1)",
            "java\tscript:alert(1)",
            "jav&#x61;script:alert(1)",
            "jav&#97;script:alert(1)",
            "javascript&colon;alert(1)",
            "vbscript:msgbox(1)",
            "data:text/html;base64,PHNjcmlwdD5hbGVydCgxKTwvc2NyaXB0Pg==",
            "file:///etc/passwd",
        ] {
            let out = sanitize(&format!("<a href=\"{href}\">x</a>"));
            assert_eq!(out, "<a>x</a>", "{href}");
        }
        assert!(safe_url("notes.md#part") && safe_url("/abs/path") && safe_url("?q=1") && safe_url("mailto:a@b.c"));
        assert!(safe_url("dir/a:b.md"));
    }

    #[test]
    fn broken_tags_become_text() {
        // A tag split over lines (block HTML comes a line at a time) can't sneak through.
        assert_eq!(sanitize("<img"), "&lt;img");
        assert_eq!(sanitize("src=x onerror=alert(1)>"), "src=x onerror=alert(1)&gt;");
        assert_eq!(sanitize("a < b > c"), "a &lt; b &gt; c");
    }
}
